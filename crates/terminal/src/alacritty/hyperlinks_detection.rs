//! Cell-level link detection, modelled one-to-one on warp-ohos'
//! `crates/warp_terminal/src/model/grid/grid_handler.rs`.
//!
//! The algorithms, the order of the checks and every constant follow warp.
//! The only deliberate difference is the grid access layer: warp owns its
//! grid and indexes it with `usize` rows from the oldest history line, while
//! alacritty indexes it with `Line(i32)` relative to the bottom of the
//! history. [`Viewport`] hides that difference by normalizing to
//! `GridPoint { row, col }` on the way in and translating back on the way out.

use std::mem;
use std::ops::RangeInclusive;

use alacritty_terminal::{
    grid::{Dimensions, Grid, Row},
    index::{Column, Line, Point as AlacPoint},
    term::cell::{Cell, Flags},
};
use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_width::UnicodeWidthChar;
use urlocator::{UrlLocation, UrlLocator};

/// Max number of characters to scan for a URL, matching warp.
const URL_SCAN_CHARACTER_MAX_COUNT: usize = 1000;

/// Max number of cells to scan per side when assembling path candidates,
/// matching warp's `LINK_NUM_CHARACTER_SCAN`.
const LINK_NUM_CHARACTER_SCAN: usize = 4096;

/// Upper bound on the number of separator-delimited fragments kept on each
/// side of the hover point, matching warp's `MAX_LINK_PATH_FRAGMENTS`.
const MAX_LINK_PATH_FRAGMENTS: usize = 32;

/// "a/" and "b/" are prefixes specific to Git Diff, stripped the same way
/// warp's `compute_valid_paths` strips them.
const PREFIXES_TO_REMOVE: [&str; 2] = ["a/", "b/"];

/// Separators are the same set warp uses. Characters beyond the explicit list
/// are covered by the Unicode general-category check in
/// [`is_file_link_separator`].
const FILE_LINK_SEPARATORS: &[char] = &[
    '\0', '\t', ' ', '(', ')', ':', '\\', ',', '"', '\'', '[', ']', '{', '}', '<', '>', ';', '|',
    '`', '=',
    // Box-drawing characters used by tree-style directory listers. warp lists
    // most of them; the remaining corners are here so a tree's guide glyph
    // never becomes part of the path.
    '│', '├', '└', '─', '┬', '┴', '┼', '║', '╠', '╚', '═', '╦', '╩', '╬', '┤', '┌', '┐', '┘',
    '┃', '┏', '┓', '┗', '┛',
];

const URL_SEPARATORS: &[char] = &[' ', '<', '>', '"', '{', '}', '|', '\\', '^', '`'];

/// Returns true when `c` should terminate a clickable URL.
///
/// URL detection allows non-ASCII letters so internationalized paths remain
/// clickable, but non-ASCII whitespace and punctuation usually indicate prose
/// around the URL.
pub(super) fn is_url_link_separator(c: char) -> bool {
    if URL_SEPARATORS.contains(&c) {
        return true;
    }
    if c.is_ascii() {
        return false;
    }
    if c.is_whitespace() {
        return true;
    }
    matches!(
        get_general_category(c),
        GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::OtherPunctuation
    )
}

/// Returns true when `c` should terminate a clickable file path.
///
/// Connectors (`Pc`) and dashes (`Pd`) are excluded since they commonly appear
/// inside identifiers and filenames.
pub(super) fn is_file_link_separator(c: char) -> bool {
    if FILE_LINK_SEPARATORS.contains(&c) {
        return true;
    }
    // The whole Box Drawing block, not just the glyphs warp happens to list:
    // tree and table output uses corners (`┤`, `╿`) that would otherwise be
    // glued onto the path.
    if ('\u{2500}'..='\u{257F}').contains(&c) {
        return true;
    }
    if c.is_ascii() {
        return false;
    }
    if c.is_whitespace() {
        return true;
    }
    matches!(
        get_general_category(c),
        GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::OtherPunctuation
    )
}

/// A grid coordinate with warp's shape: `row` counts from the oldest history
/// line, `col` from the left edge.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct GridPoint {
    pub(super) row: usize,
    pub(super) col: usize,
}

impl GridPoint {
    fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// Cell classification used by the cursor to skip wide-character spacers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CellType {
    RegularChar,
    WideChar,
    WideCharSpacer,
    LeadingWideCharSpacer,
}

/// Read-only view of an alacritty grid using warp's coordinate model.
pub(super) struct Viewport<'a> {
    grid: &'a Grid<Cell>,
    first_line: i32,
    columns: usize,
    total_rows: usize,
}

impl<'a> Viewport<'a> {
    pub(super) fn new(grid: &'a Grid<Cell>) -> Self {
        let first_line = grid.topmost_line().0;
        let last_line = grid.bottommost_line().0;
        let total_rows = (last_line - first_line + 1).max(0) as usize;
        Self {
            grid,
            first_line,
            columns: grid.columns(),
            total_rows,
        }
    }

    pub(super) fn point(&self, point: AlacPoint) -> GridPoint {
        GridPoint::new(
            (point.line.0 - self.first_line).max(0) as usize,
            point.column.0,
        )
    }

    pub(super) fn alacritty_point(&self, point: GridPoint) -> AlacPoint {
        AlacPoint::new(
            Line(self.first_line + point.row as i32),
            Column(point.col),
        )
    }

    fn row(&self, index: usize) -> Option<&'a Row<Cell>> {
        if index >= self.total_rows {
            return None;
        }
        Some(&self.grid[Line(self.first_line + index as i32)])
    }

    fn row_wraps(&self, index: usize) -> bool {
        let Some(row) = self.row(index) else {
            return false;
        };
        // alacritty rows are variable length, so a soft-wrapped row can end
        // before the last column. Check both the last stored cell and the last
        // column: the flag sits on the cell that ended the row.
        let last_stored = row.len().checked_sub(1).unwrap_or(0);
        let last_column = self.columns.saturating_sub(1);
        [last_stored, last_column].into_iter().any(|col| {
            row_cell(row, col).is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE))
        })
    }

    fn columns(&self) -> usize {
        self.columns
    }

    fn total_rows(&self) -> usize {
        self.total_rows
    }

    fn cell_type(&self, point: GridPoint) -> Option<CellType> {
        if point.row >= self.total_rows || point.col >= self.columns {
            return None;
        }
        // alacritty rows are variable length: a column past the stored content
        // is a blank cell. warp's grid is fixed size, so its `cell_type` reports
        // the same column as a regular character; returning `None` here would
        // make `advance_point_by_columns` stop at the end of a row instead of
        // continuing into the next one.
        match row_cell(self.row(point.row)?, point.col) {
            Some(cell) => Some(cell_type(cell)),
            None => Some(CellType::RegularChar),
        }
    }

    /// Number of cells that carry content, matching warp's `LineLength`.
    ///
    /// A row ending in `WRAPLINE` is full. Otherwise trailing default cells are
    /// excluded so hovering past the end of a line is not a link probe.
    fn line_length(&self, index: usize) -> usize {
        let Some(row) = self.row(index) else {
            return 0;
        };
        let len = row.len();
        if len == 0 {
            return 0;
        }
        if row_cell(row, len - 1).is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE)) {
            return len;
        }

        for index in (0..len).rev() {
            if row_cell(row, index).is_some_and(|cell| cell.c != ' ') {
                return index + 1;
            }
        }
        0
    }

    fn grapheme_cursor_from(&self, point: GridPoint, wrap: Wrap) -> GridCursor<'_, 'a> {
        GridCursor::new(self, point, wrap)
    }
}

/// Returns the cell at `col`, or `None` when the column is past the row's
/// stored content. alacritty's `Row` only exposes `Index`, which panics out of
/// bounds, so every access goes through this helper.
fn row_cell(row: &Row<Cell>, col: usize) -> Option<&Cell> {
    (col < row.len()).then(|| &row[Column(col)])
}

/// Display width of a cell, taken from its flags rather than recomputing it
/// from the character: alacritty already decided the width when it wrote it.
fn cell_width(cell: &Cell) -> usize {
    if cell.flags.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    }
}

fn cell_type(cell: &Cell) -> CellType {
    if !cell
        .flags
        .intersects(Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
    {
        CellType::RegularChar
    } else if cell.flags.intersects(Flags::WIDE_CHAR) {
        CellType::WideChar
    } else if cell.flags.intersects(Flags::WIDE_CHAR_SPACER) {
        CellType::WideCharSpacer
    } else {
        CellType::LeadingWideCharSpacer
    }
}

/// How a cursor crosses row boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wrap {
    /// Does not wrap at all (stops at the start and end of a row).
    None,
    /// Wraps at the row boundaries, but stops upon reaching a newline.
    Soft,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorDirection {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorState {
    Valid,
    Exhausted(CursorDirection),
}

struct CursorItem<'a> {
    cell: &'a Cell,
    point: GridPoint,
}

impl CursorItem<'_> {
    fn cell(&self) -> &Cell {
        self.cell
    }

    fn point(&self) -> GridPoint {
        self.point
    }
}

/// Iterates graphemes in a terminal grid, skipping wide-character spacers.
///
/// Mirrors warp's `GraphemeCursor`. The only difference is the row cache
/// borrows from the grid directly instead of a `Cow`.
struct GridCursor<'v, 'g> {
    viewport: &'v Viewport<'g>,
    cur: GridPoint,
    cursor_state: CursorState,
    wrap: Wrap,
    cached_row_idx: usize,
    cached_row: Option<&'g Row<Cell>>,
}

impl<'v, 'g> GridCursor<'v, 'g> {
    fn new(viewport: &'v Viewport<'g>, point: GridPoint, wrap: Wrap) -> Self {
        let cached_row = viewport.row(point.row);
        let mut cursor = Self {
            viewport,
            cur: point,
            cursor_state: CursorState::Valid,
            wrap,
            cached_row_idx: point.row,
            cached_row,
        };

        // The cursor should never start on a spacer cell. If we're on the
        // second cell of a wide char, move to the first cell; if we're on the
        // cell where a wide char should have started, move to its actual start.
        let flags = cursor
            .cached_row
            .and_then(|row| row_cell(row, point.col))
            .map(|cell| cell.flags)
            .unwrap_or_else(Flags::empty);
        if flags.intersects(Flags::WIDE_CHAR_SPACER) {
            cursor.move_backward();
        } else if flags.intersects(Flags::LEADING_WIDE_CHAR_SPACER) {
            cursor.move_forward();
        }

        cursor
    }

    fn current_item(&self) -> Option<CursorItem<'_>> {
        if self.cursor_state != CursorState::Valid {
            return None;
        }
        let cell = row_cell(self.cached_row?, self.cur.col)?;
        Some(CursorItem {
            cell,
            point: self.cur,
        })
    }

    fn current_point_valid(&self) -> bool {
        self.cur.row < self.viewport.total_rows() && self.cur.col < self.viewport.columns()
    }

    fn has_next(&self) -> bool {
        if !self.current_point_valid() {
            return false;
        }

        let at_end_of_row = self.is_at_end_of_row();
        if self.wrap == Wrap::None && at_end_of_row
            || self.wrap == Wrap::Soft && self.is_at_end_of_line()
        {
            return false;
        }

        !at_end_of_row || self.cur.row < self.viewport.total_rows() - 1
    }

    fn has_prev(&self) -> bool {
        if !self.current_point_valid() {
            return false;
        }

        let at_start_of_row = self.is_at_start_of_row();
        if self.wrap == Wrap::None && at_start_of_row
            || self.wrap == Wrap::Soft && self.is_at_start_of_line()
        {
            return false;
        }

        !at_start_of_row || self.cur.row > 0
    }

    fn update_cached_row(&mut self) {
        self.cached_row_idx = self.cur.row;
        self.cached_row = self.viewport.row(self.cached_row_idx);
    }

    fn move_forward(&mut self) {
        match self.cursor_state {
            CursorState::Valid if self.has_next() => {
                let start_row = self.cur.row;
                self.cur = self.wrapping_add(self.cur, self.viewport.columns(), 1);
                if matches!(
                    self.viewport.cell_type(self.cur),
                    Some(CellType::WideCharSpacer | CellType::LeadingWideCharSpacer)
                ) {
                    self.move_forward();
                }

                if self.cur.row != start_row {
                    self.update_cached_row();
                }
            }
            CursorState::Valid => {
                self.cursor_state = CursorState::Exhausted(CursorDirection::Right);
            }
            CursorState::Exhausted(CursorDirection::Left) => {
                self.cursor_state = CursorState::Valid;
            }
            CursorState::Exhausted(CursorDirection::Right) => (),
        }
    }

    fn move_backward(&mut self) {
        match self.cursor_state {
            CursorState::Valid if self.has_prev() => {
                let start_row = self.cur.row;
                self.cur = self.wrapping_sub(self.cur, self.viewport.columns(), 1);
                if matches!(
                    self.viewport.cell_type(self.cur),
                    Some(CellType::WideCharSpacer | CellType::LeadingWideCharSpacer)
                ) {
                    self.move_backward();
                }

                if self.cur.row != start_row {
                    self.update_cached_row();
                }
            }
            CursorState::Valid => {
                self.cursor_state = CursorState::Exhausted(CursorDirection::Left);
            }
            CursorState::Exhausted(CursorDirection::Right) => {
                self.cursor_state = CursorState::Valid;
            }
            CursorState::Exhausted(CursorDirection::Left) => (),
        }
    }

    fn is_at_start_of_row(&self) -> bool {
        self.cur.col == 0
    }

    fn is_at_end_of_row(&self) -> bool {
        self.cur.col == self.viewport.columns().saturating_sub(1)
    }

    /// True when moving backwards would cross a hard wrap.
    fn is_at_start_of_line(&self) -> bool {
        self.is_at_start_of_row()
            && (self.cur.row == 0 || !self.viewport.row_wraps(self.cur.row - 1))
    }

    /// True when moving forwards would cross a hard wrap.
    fn is_at_end_of_line(&self) -> bool {
        self.is_at_end_of_row()
            && (self.cur.row == self.viewport.total_rows() - 1
                || !self.viewport.row_wraps(self.cur.row))
    }

    fn wrapping_add(&self, point: GridPoint, columns: usize, count: usize) -> GridPoint {
        let new_col = point.col + count;
        GridPoint::new(point.row + new_col / columns, new_col % columns)
    }

    fn wrapping_sub(&self, point: GridPoint, columns: usize, count: usize) -> GridPoint {
        let total = point.row * columns + point.col;
        let new_total = total.saturating_sub(count);
        GridPoint::new(new_total / columns, new_total % columns)
    }
}

/// A contiguous span of grid cells.
#[derive(Clone, Debug)]
struct Link {
    range: RangeInclusive<GridPoint>,
    is_empty: bool,
}

impl Link {
    fn empty(point: GridPoint) -> Self {
        Self {
            range: RangeInclusive::new(point, point),
            is_empty: true,
        }
    }

    fn extend_link(&mut self, point: GridPoint) {
        if self.is_empty {
            self.range = RangeInclusive::new(point, point);
            self.is_empty = false;
        } else {
            self.range = RangeInclusive::new(*self.range.start(), point);
        }
    }
}

/// Detects an OSC 8 hyperlink at `point` and returns its span plus its URI.
///
/// Mirrors warp's `hyperlink_at_point`: walk left and right while the adjacent
/// cell carries the same hyperlink id, across soft wraps.
pub(super) fn hyperlink_at_point(
    viewport: &Viewport<'_>,
    point: GridPoint,
) -> Option<(GridPoint, GridPoint, String)> {
    let row = viewport.row(point.row)?;
    if point.col >= viewport.line_length(point.row) {
        return None;
    }
    let target_id = row_cell(row, point.col)?.hyperlink()?.id().to_owned();

    let mut start_point = point;
    let mut back_cursor = viewport.grapheme_cursor_from(point, Wrap::Soft);
    back_cursor.move_backward();
    while let Some(item) = back_cursor.current_item() {
        let matches = item
            .cell()
            .hyperlink()
            .is_some_and(|hyperlink| hyperlink.id() == target_id);
        if !matches {
            break;
        }
        start_point = item.point();
        back_cursor.move_backward();
    }

    let mut end_point = point;
    let mut forward_cursor = viewport.grapheme_cursor_from(point, Wrap::Soft);
    forward_cursor.move_forward();
    while let Some(item) = forward_cursor.current_item() {
        let matches = item
            .cell()
            .hyperlink()
            .is_some_and(|hyperlink| hyperlink.id() == target_id);
        if !matches {
            break;
        }
        end_point = item.point();
        forward_cursor.move_forward();
    }

    let uri = row_cell(row, point.col)?.hyperlink()?.uri().to_owned();
    Some((start_point, end_point, uri))
}

/// Detects a URL under `point`, mirroring warp's `url_at_point` step by step.
pub(super) fn url_at_point(
    viewport: &Viewport<'_>,
    original_point: GridPoint,
) -> Option<(GridPoint, GridPoint)> {
    let row = viewport.row(original_point.row)?;
    let line_length = viewport.line_length(original_point.row);

    // Omit cases when the user is hovering over blank spaces after the end of
    // a line.
    if original_point.col >= line_length {
        return None;
    }

    let current_cell = row_cell(row, original_point.col)?;

    let is_at_boundary = |cell: &Cell| {
        !cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            && is_url_link_separator(cell.c)
    };
    // If the point is on a separator, return directly because this can't be
    // part of a url.
    if is_at_boundary(current_cell) {
        return None;
    }

    // Scan backward until the fragment boundary.
    let mut cursor = viewport.grapheme_cursor_from(original_point, Wrap::Soft);
    cursor.move_backward();

    let mut starting_point = original_point;
    let mut total_characters_scanned = 0;

    while let Some(item) = cursor.current_item() {
        let current_point = item.point();

        if is_at_boundary(item.cell()) {
            break;
        }

        if total_characters_scanned > URL_SCAN_CHARACTER_MAX_COUNT {
            return None;
        }

        starting_point = current_point;
        total_characters_scanned += 1;
        cursor.move_backward();
    }

    let mut cursor = viewport.grapheme_cursor_from(starting_point, Wrap::Soft);
    let mut locator = UrlLocator::new();
    let mut state = UrlLocation::Reset;
    let mut scheme_buffer = Vec::new();
    let mut url = Link::empty(GridPoint::default());

    let mut passed_point = false;
    total_characters_scanned = 0;
    while let Some(item) = cursor.current_item() {
        if total_characters_scanned > URL_SCAN_CHARACTER_MAX_COUNT {
            url = Link::empty(GridPoint::default());
            break;
        }
        let current_point = item.point();

        if current_point >= original_point {
            passed_point = true;
        }

        if is_at_boundary(item.cell()) {
            break;
        }

        let last_state = mem::replace(&mut state, locator.advance(item.cell().c));
        let link_changed = match (state, last_state) {
            (UrlLocation::Url(_length, _num_illegal_end_chars), UrlLocation::Scheme) => {
                url = Link::empty(GridPoint::default());
                for scheme_point in &scheme_buffer {
                    url.extend_link(*scheme_point);
                }
                url.extend_link(current_point);
                true
            }
            (UrlLocation::Url(_length, num_illegal_end_chars), UrlLocation::Url(..)) => {
                if num_illegal_end_chars == 0 {
                    url.extend_link(current_point);
                }
                true
            }
            (UrlLocation::Scheme, _) => {
                scheme_buffer.push(current_point);
                true
            }
            (UrlLocation::Reset, _) => {
                locator = UrlLocator::new();
                state = UrlLocation::Reset;
                scheme_buffer.clear();
                false
            }
            _ => false,
        };

        // We are at the hovered point and the link has not updated -- the point
        // is not part of a url.
        if current_point == original_point && !link_changed {
            return None;
        // Passed the hovered point and the link hasn't changed -- break because
        // all the later urls will not include the point.
        } else if passed_point && !link_changed {
            break;
        }

        cursor.move_forward();
        total_characters_scanned += 1;
    }

    if url.is_empty || !url.range.contains(&original_point) {
        None
    } else {
        Some((*url.range.start(), *url.range.end()))
    }
}

/// A fragment of a row: a run of cells without separators, or a single
/// separator.
#[derive(Debug, PartialEq, Eq)]
struct Fragment {
    content: String,
    total_cell_width: usize,
}

impl Fragment {
    fn has_separator(&self) -> bool {
        self.content.chars().any(is_file_link_separator)
    }
}

/// Joins fragments that were split by a soft wrap, mirroring warp.
fn append_fragments_across_soft_wrap(fragments: &mut Vec<Fragment>, mut next: Vec<Fragment>) {
    match (fragments.last(), next.first()) {
        (Some(last), Some(first)) if !last.has_separator() && !first.has_separator() => {
            let mut fragment = fragments.pop().expect("fragment should exist");
            fragment.content.push_str(&first.content);
            fragment.total_cell_width += first.total_cell_width;
            fragments.push(fragment);
            next.remove(0);
        }
        _ => {}
    }

    fragments.append(&mut next);
}

/// Pushes a cell's visible content, including any zero-width characters.
fn push_cell_content(out: &mut String, cell: &Cell) {
    out.push(cell.c);
    if let Some(zerowidth) = cell.zerowidth() {
        out.extend(zerowidth);
    }
}

/// Splits a row range into separator-delimited fragments, mirroring warp's
/// `line_to_fragments`.
///
/// warp also expands tab cells using its tab-stop bitmap; alacritty expands
/// tabs into spaces when they are written (`put_tab`), so a `'\t'` cell never
/// reaches the grid and that branch is not needed here.
fn line_to_fragments(
    viewport: &Viewport<'_>,
    row_index: usize,
    cols: std::ops::Range<usize>,
    include_first_wide_char: bool,
) -> Vec<Fragment> {
    let Some(grid_line) = viewport.row(row_index) else {
        return Vec::new();
    };

    let mut fragments = Vec::new();
    let line_length = viewport.line_length(row_index).min(cols.end + 1);

    let mut last_fragment = String::new();
    let mut last_fragment_length = 0;

    let mut cols = cols;
    if include_first_wide_char
        && row_cell(grid_line, cols.start)
            .is_some_and(|cell| cell.flags.contains(Flags::WIDE_CHAR_SPACER))
    {
        cols.start = cols.start.saturating_sub(1);
    }

    for col in cols.start..line_length {
        let Some(cell) = row_cell(grid_line, col) else {
            return Vec::new();
        };

        if !cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            if is_file_link_separator(cell.c) {
                if !last_fragment.is_empty() {
                    let mut fragment_text = String::new();
                    mem::swap(&mut fragment_text, &mut last_fragment);

                    fragments.push(Fragment {
                        content: fragment_text,
                        total_cell_width: last_fragment_length,
                    });

                    last_fragment.clear();
                    last_fragment_length = 0;
                }

                fragments.push(Fragment {
                    content: cell.c.into(),
                    total_cell_width: cell_width(cell),
                });
            } else {
                push_cell_content(&mut last_fragment, cell);
                last_fragment_length += 1;
            }
        } else if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
            || (cell.flags.contains(Flags::WIDE_CHAR_SPACER) && !last_fragment.is_empty())
        {
            last_fragment_length += 1
        }
    }

    // Make sure the last cell's width accounts for a trailing wide-char spacer
    // that `line_length` excludes.
    if line_length < grid_line.len()
        && !last_fragment.is_empty()
        && row_cell(grid_line, line_length)
            .is_some_and(|cell| cell.flags.contains(Flags::WIDE_CHAR_SPACER))
    {
        last_fragment_length += 1;
    }

    if !last_fragment.is_empty() {
        fragments.push(Fragment {
            content: last_fragment.clone(),
            total_cell_width: last_fragment_length,
        });
    }

    fragments
}

/// A path candidate found around the pointer.
#[derive(Debug)]
pub(super) struct PossiblePath {
    pub(super) text: String,
    pub(super) start: GridPoint,
    pub(super) end: GridPoint,
}

/// Returns every possible file path containing `point`, ordered the same way
/// warp orders them (nearest-then-longer combinations, finally reversed).
pub(super) fn possible_file_paths_at_point(
    viewport: &Viewport<'_>,
    point: GridPoint,
) -> Vec<PossiblePath> {
    // Snap a pointer that landed on a wide character's spacer back onto the
    // cell that owns the character, the same adjustment `GridCursor` makes.
    let point = match viewport.cell_type(point) {
        Some(CellType::WideCharSpacer) => GridPoint::new(point.row, point.col.saturating_sub(1)),
        Some(CellType::LeadingWideCharSpacer) => GridPoint::new(
            point.row,
            (point.col + 1).min(viewport.columns().saturating_sub(1)),
        ),
        _ => point,
    };

    let columns = viewport.columns();
    if columns == 0 {
        return Vec::new();
    }

    // All fragments in the rows before the point (not including the point)
    // plus fragments in earlier soft-wrapped rows.
    let mut prefix_chunks = Vec::new();
    let mut first_prefix_row = point.row;
    let mut scanned_prefix_width = point.col;
    while first_prefix_row > 0
        && viewport.row_wraps(first_prefix_row - 1)
        && scanned_prefix_width < LINK_NUM_CHARACTER_SCAN
    {
        first_prefix_row -= 1;
        scanned_prefix_width += columns;
    }

    for row in first_prefix_row..=point.row {
        let range = if row == point.row {
            if point.col == 0 {
                continue;
            }
            0..point.col - 1
        } else if row == first_prefix_row {
            let extra_width = scanned_prefix_width.saturating_sub(LINK_NUM_CHARACTER_SCAN);
            extra_width.min(columns.saturating_sub(1))..columns - 1
        } else {
            0..columns - 1
        };

        let fragments = line_to_fragments(viewport, row, range, true);
        append_fragments_across_soft_wrap(&mut prefix_chunks, fragments);
    }

    // All fragments in the rows after the point (including the point) plus
    // fragments in later soft-wrapped rows.
    let mut suffix_chunks = Vec::new();
    let mut last_suffix_row = point.row;
    let mut scanned_suffix_width = columns.saturating_sub(point.col);
    while last_suffix_row + 1 < viewport.total_rows()
        && viewport.row_wraps(last_suffix_row)
        && scanned_suffix_width < LINK_NUM_CHARACTER_SCAN
    {
        last_suffix_row += 1;
        scanned_suffix_width += columns;
    }

    for row in point.row..=last_suffix_row {
        let range = if row == point.row {
            point.col..columns - 1
        } else if row == last_suffix_row {
            let extra_width = scanned_suffix_width.saturating_sub(LINK_NUM_CHARACTER_SCAN);
            0..(columns - 1).saturating_sub(extra_width)
        } else {
            0..columns - 1
        };

        let fragments = line_to_fragments(viewport, row, range, false);
        append_fragments_across_soft_wrap(&mut suffix_chunks, fragments);
    }

    // Bound the number of fragment combinations the candidate search
    // considers, dropping the fragments farthest from the pointer.
    if prefix_chunks.len() > MAX_LINK_PATH_FRAGMENTS {
        let excess = prefix_chunks.len() - MAX_LINK_PATH_FRAGMENTS;
        prefix_chunks.drain(0..excess);
    }
    suffix_chunks.truncate(MAX_LINK_PATH_FRAGMENTS);

    // A path that starts at the point is built from suffix chunks alone, which
    // is only possible when the last prefix fragment is a separator (or the
    // prefix is empty). Push a dummy fragment so one pass covers it.
    let should_check_suffix = match prefix_chunks.last() {
        Some(last_prefix) => last_prefix
            .content
            .chars()
            .next()
            .map(is_file_link_separator)
            .unwrap_or(false),
        None => true,
    };

    if should_check_suffix {
        prefix_chunks.push(Fragment {
            content: String::new(),
            total_cell_width: 0,
        });
    }

    let mut left = String::new();
    let mut left_width = 0;
    let mut possible_paths = Vec::new();

    for prefix_chunk in prefix_chunks.into_iter().rev() {
        left = format!("{}{}", prefix_chunk.content, left);
        left_width += prefix_chunk.total_cell_width;

        let mut right = String::new();
        let mut right_width = 0;

        for suffix_chunk in suffix_chunks.iter() {
            right.push_str(suffix_chunk.content.as_str());
            right_width += suffix_chunk.total_cell_width;

            let possible_path = format!("{left}{right}");

            let starting_point = advance_point_by_columns(
                viewport,
                point,
                left_width,
                false, /* forward */
            );
            let ending_point = advance_point_by_columns(
                viewport,
                point,
                right_width.saturating_sub(1),
                true, /* forward */
            );

            possible_paths.push(PossiblePath {
                text: possible_path,
                start: starting_point,
                end: ending_point,
            });
        }
    }

    // warp reverses the raw candidates and then, for each one, tries the
    // punctuation-trimmed spelling, the raw spelling, a Git Diff prefix and the
    // Git Bash symlink marker. Expand after reversing so the first candidate
    // that resolves is the one warp would have picked.
    possible_paths.reverse();

    let mut expanded = Vec::new();
    for path in possible_paths {
        push_path_variants(viewport, &mut expanded, path.text, path.start, path.end);
    }
    expanded
}

/// Pushes the spellings warp's `compute_valid_paths` tries for one candidate.
fn push_path_variants(
    viewport: &Viewport<'_>,
    out: &mut Vec<PossiblePath>,
    text: String,
    start: GridPoint,
    end: GridPoint,
) {
    // Prose punctuation that trails a path is tried first, so the resolved
    // file, the highlight range and the extension all exclude it.
    if let Some((trimmed, removed_width)) = path_without_trailing_sentence_punctuation(&text) {
        let trimmed_end = advance_point_by_columns(viewport, end, removed_width, false);
        out.push(PossiblePath {
            text: trimmed.to_owned(),
            start,
            end: trimmed_end,
        });
    }

    out.push(PossiblePath {
        text: text.clone(),
        start,
        end,
    });

    for prefix in PREFIXES_TO_REMOVE {
        if let Some(stripped) = text.strip_prefix(prefix) {
            let prefix_width = prefix
                .chars()
                .map(|c| UnicodeWidthChar::width(c).unwrap_or(1))
                .sum();
            let stripped_start = advance_point_by_columns(viewport, start, prefix_width, true);
            out.push(PossiblePath {
                text: stripped.to_owned(),
                start: stripped_start,
                end,
            });
        }
    }

    // Git Bash marks symlinks by appending `@` when listing a directory.
    if let Some(stripped) = text.strip_suffix('@') {
        let stripped_end = advance_point_by_columns(viewport, end, 1, false);
        out.push(PossiblePath {
            text: stripped.to_owned(),
            start,
            end: stripped_end,
        });
    }
}

/// Strips trailing sentence punctuation, mirroring warp's
/// `path_without_trailing_sentence_punctuation`.
///
/// A trailing period that belongs to a `.` / `..` path component is preserved.
fn path_without_trailing_sentence_punctuation(path: &str) -> Option<(&str, usize)> {
    fn is_trailing_sentence_punctuation(c: char) -> bool {
        if c == '.' {
            return true;
        }
        if c.is_ascii() {
            return false;
        }
        matches!(
            get_general_category(c),
            GeneralCategory::ClosePunctuation
                | GeneralCategory::FinalPunctuation
                | GeneralCategory::OtherPunctuation
        )
    }

    let mut trimmed = path;
    let mut removed_width = 0;

    while let Some(c) = trimmed.chars().next_back() {
        if !is_trailing_sentence_punctuation(c) {
            break;
        }

        let Some(shorter) = trimmed.strip_suffix(c) else {
            break;
        };
        if shorter.is_empty() {
            break;
        }

        if c == '.' {
            match shorter.chars().next_back() {
                None | Some('.') | Some('/') | Some('\\') => break,
                _ => {}
            }
        }

        trimmed = shorter;
        // The highlight range is measured in cells, so a wide punctuation mark
        // (`。`, `）`) must remove both of its columns.
        removed_width += UnicodeWidthChar::width(c).unwrap_or(1);
    }

    (removed_width > 0).then_some((trimmed, removed_width))
}

/// Advances `count` cells from `point`, wrapping across rows.
fn advance_point_by_columns(
    viewport: &Viewport<'_>,
    point: GridPoint,
    count: usize,
    forward: bool,
) -> GridPoint {
    if count == 0 {
        return point;
    }

    // warp walks one cell at a time and stops when the cursor leaves the grid,
    // so the result is linear arithmetic. Doing it in one step keeps path
    // detection linear in the number of candidates instead of quadratic in the
    // length of a long line.
    //
    // The clamp differs from warp by one cell at the very end of the grid: warp
    // returns the first out-of-range point, while this stays on the last cell
    // so callers never translate a `Line` past the bottom of the history.
    let columns = viewport.columns();
    let total = point.row * columns + point.col;
    let last = (viewport.total_rows() * columns).saturating_sub(1);
    let advanced = if forward {
        total.saturating_add(count).min(last)
    } else {
        total.saturating_sub(count)
    };
    GridPoint::new(advanced / columns, advanced % columns)
}


