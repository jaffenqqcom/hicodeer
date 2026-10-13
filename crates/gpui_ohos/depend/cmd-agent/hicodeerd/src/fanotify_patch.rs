//! Point-fix for Go programs whose startup fanotify probe is fatal on OHOS.
//!
//! OHOS's seccomp filter answers `fanotify_init` with SIGSYS instead of an
//! error code, so a program that probes it while its packages initialise dies
//! before it can fall back to another watcher. typescript-go -- the native
//! TypeScript language server `tsc` -- does exactly that, and its probe lives
//! in one function, `fswatch.fanotifyAvailable`. Rewriting that function to
//! return false is equivalent to the `GOOS == "android"` short-circuit the
//! upstream source already carries.
//!
//! The function is located through the Go symbol table in `.gopclntab`, which
//! survives stripping, so nothing here assumes a fixed address and a rebuilt
//! program is handled the same way. Everything is best effort: a non-Go
//! binary, an unfamiliar table layout, or an already rewritten function all
//! leave the file untouched.
//!
//! Rewriting changes the bytes, so a signature the file already carried stops
//! matching; the caller must sign afterwards, in that order.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Function typescript-go probes fanotify from. Matched as a suffix because the
/// full name carries the module path.
const PROBE_FUNCTION_SUFFIX: &str = "fswatch.fanotifyAvailable";
/// aarch64 encoding of `mov w0, #0` then `ret`: the function returns false.
const RETURN_FALSE: [u8; 8] = [0x00, 0x00, 0x80, 0x52, 0xc0, 0x03, 0x5f, 0xd6];
/// Section holding the pc-to-function table.
const PCLNTAB_SECTION: &str = ".gopclntab";
/// Section that function entry offsets are relative to.
const TEXT_SECTION: &str = ".text";
/// Magic of the pcHeader layout this module understands (go1.20 and later).
const PCLNTAB_MAGIC: u32 = 0xffff_fff1;
/// Bounds so a corrupt or hostile header cannot make us allocate or loop
/// wildly: how many sections and functions to accept, and how large the
/// section-name table may be.
const MAX_SECTION_COUNT: u64 = 4096;
const MAX_FUNCTION_COUNT: u64 = 1 << 20;
const MAX_STRTAB_BYTES: u64 = 1 << 20;
/// Cap on the pc-to-function table a binary may claim to carry. The table is
/// read into memory whole, so a header naming an enormous size would be an
/// out-of-memory abort rather than a missed patch.
const MAX_PCLNTAB_BYTES: u64 = 1 << 26;

/// One section's file offset and size.
struct Section {
    offset: u64,
    size: u64,
}

/// The two sections this module needs from the ELF.
struct Sections {
    text: Section,
    pclntab: Section,
}

/// The outcome of inspecting one file for the fanotify probe.
pub enum PatchOutcome {
    /// The probe was found and rewritten to return false.
    Rewritten,
    /// The probe was found and already returns false.
    AlreadyPatched,
    /// No fanotify probe here: the file is not one this daemon prepares. Such a
    /// file -- a system binary like /bin/sh, say -- must be left entirely
    /// alone, its signature included.
    NotTarget,
}

/// Rewrites the fanotify probe in `path` to return false, reporting what was
/// found. See the module note: sign afterwards whenever this reports a rewrite,
/// and only then -- a file with no probe is not ours to sign either.
pub fn patch_if_needed(path: &Path) -> PatchOutcome {
    let Ok(mut file) = OpenOptions::new().read(true).write(true).open(path) else {
        return PatchOutcome::NotTarget;
    };
    let Some(sections) = read_sections(&mut file) else {
        return PatchOutcome::NotTarget;
    };
    let Some(entry_offset) = probe_entry_offset(&mut file, &sections.pclntab) else {
        return PatchOutcome::NotTarget;
    };
    // The rewrite writes RETURN_FALSE.len() bytes, so the whole span must lie
    // inside .text -- an entry near its end would otherwise spill into the
    // section that follows and corrupt it.
    let Some(end) = entry_offset.checked_add(RETURN_FALSE.len() as u64) else {
        return PatchOutcome::NotTarget;
    };
    if end > sections.text.size {
        return PatchOutcome::NotTarget;
    }
    let Some(at) = sections.text.offset.checked_add(entry_offset) else {
        return PatchOutcome::NotTarget;
    };
    let mut current = [0u8; RETURN_FALSE.len()];
    if read_at(&mut file, at, &mut current).is_none() {
        return PatchOutcome::NotTarget;
    }
    if current == RETURN_FALSE {
        // Already rewritten, so nothing left to do.
        return PatchOutcome::AlreadyPatched;
    }
    if write_at(&mut file, at, &RETURN_FALSE) {
        PatchOutcome::Rewritten
    } else {
        PatchOutcome::NotTarget
    }
}

/// Reads `.text` and `.gopclntab` out of the ELF section table. `None` for
/// anything that is not a 64-bit little-endian ELF carrying both, which is
/// every binary this module must leave alone.
fn read_sections(file: &mut File) -> Option<Sections> {
    let mut header = [0u8; 64];
    read_at(file, 0, &mut header)?;
    // ELF magic, then ELFCLASS64 and ELFDATA2LSB.
    if header[..4] != [0x7f, b'E', b'L', b'F'] || header[4] != 2 || header[5] != 1 {
        return None;
    }
    let section_table = read_u64(&header, 0x28)?;
    let entry_size = u64::from(read_u16(&header, 0x3a)?);
    let section_count = u64::from(read_u16(&header, 0x3c)?);
    let names_index = u64::from(read_u16(&header, 0x3e)?);
    if section_table == 0
        || section_count == 0
        || section_count > MAX_SECTION_COUNT
        || entry_size < 64
        || names_index >= section_count
    {
        return None;
    }
    let names_header = section_table.checked_add(names_index.checked_mul(entry_size)?)?;
    let (_, names_offset, names_size) = read_section_header(file, names_header)?;
    if names_size == 0 || names_size > MAX_STRTAB_BYTES {
        return None;
    }
    let mut names = vec![0u8; names_size as usize];
    read_at(file, names_offset, &mut names[..])?;
    let mut text = None;
    let mut pclntab = None;
    for index in 0..section_count {
        let at = section_table.checked_add(index.checked_mul(entry_size)?)?;
        let Some((name_index, offset, size)) = read_section_header(file, at) else {
            continue;
        };
        let Some(name) = cstr_at(&names, name_index as usize) else {
            continue;
        };
        if name == TEXT_SECTION {
            text = Some(Section { offset, size });
        } else if name == PCLNTAB_SECTION {
            pclntab = Some(Section { offset, size });
        }
    }
    Some(Sections {
        text: text?,
        pclntab: pclntab?,
    })
}

/// The entry offset of the probe function within `.text`, or `None` when the
/// pc-to-function table does not name it.
fn probe_entry_offset(file: &mut File, pclntab: &Section) -> Option<u64> {
    if pclntab.size == 0 || pclntab.size > MAX_PCLNTAB_BYTES {
        return None;
    }
    let mut table = vec![0u8; pclntab.size as usize];
    read_at(file, pclntab.offset, &mut table[..])?;
    if read_u32(&table, 0)? != PCLNTAB_MAGIC || *table.get(7)? != 8 {
        return None;
    }
    let function_count = read_u64(&table, 8)?;
    let function_names_offset = read_u64(&table, 32)?;
    let function_table_offset = read_u64(&table, 64)?;
    if function_count == 0 || function_count > MAX_FUNCTION_COUNT {
        return None;
    }
    // Cheap rejection: most Go binaries name no fanotify function at all, and
    // the whole name table can be searched before touching the entries.
    let names = table.get(function_names_offset as usize..)?;
    if !contains(names, PROBE_FUNCTION_SUFFIX.as_bytes()) {
        return None;
    }
    let function_table = function_table_offset as usize;
    for index in 0..function_count {
        let at = function_table.checked_add((index as usize).checked_mul(8)?)?;
        let entry_offset = read_u32(&table, at)?;
        let function_offset = read_u32(&table, at.checked_add(4)?)?;
        let name_field = function_table.checked_add(function_offset as usize)?;
        let name_offset = read_i32(&table, name_field.checked_add(4)?)?;
        let name_at = function_names_offset.checked_add(name_offset.max(0) as u64)?;
        let Some(name) = cstr_at(&table, name_at as usize) else {
            continue;
        };
        if name.ends_with(PROBE_FUNCTION_SUFFIX) {
            return Some(u64::from(entry_offset));
        }
    }
    None
}

/// Reads a section header's name index, file offset, and size.
fn read_section_header(file: &mut File, at: u64) -> Option<(u32, u64, u64)> {
    let mut bytes = [0u8; 64];
    read_at(file, at, &mut bytes)?;
    Some((
        read_u32(&bytes, 0)?,
        read_u64(&bytes, 0x18)?,
        read_u64(&bytes, 0x20)?,
    ))
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let field = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([field[0], field[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn read_i32(bytes: &[u8], at: usize) -> Option<i32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(i32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let field = bytes.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(field.try_into().ok()?))
}

/// The NUL-terminated UTF-8 string starting at `at`, when one is there.
fn cstr_at(bytes: &[u8], at: usize) -> Option<&str> {
    let rest = bytes.get(at..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    std::str::from_utf8(&rest[..end]).ok()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|window| window == needle)
}

fn read_at(file: &mut File, at: u64, buffer: &mut [u8]) -> Option<()> {
    file.seek(SeekFrom::Start(at)).ok()?;
    file.read_exact(buffer).ok()?;
    Some(())
}

fn write_at(file: &mut File, at: u64, buffer: &[u8]) -> bool {
    file.seek(SeekFrom::Start(at)).is_ok() && file.write_all(buffer).is_ok() && file.flush().is_ok()
}
