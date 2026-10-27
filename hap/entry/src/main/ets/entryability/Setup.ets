import common from '@ohos.app.ability.common';
import fs from '@ohos.file.fs';
import fileUri from '@ohos.file.fileuri';
import fileShare from '@ohos.fileshare';
import picker from '@ohos.file.picker';
import hilog from '@ohos.hilog';

const DOMAIN: number = 0x0001;
const TAG: string = 'Setup';
/** Named route loaded by `windowStage.loadContentByName`. */
export const RouteName: string = 'Setup';
/** File in the HAP private files dir holding the chosen home directory path. */
const RECORD_FILE: string = 'custom_data_dir';
/** Paths under this prefix live outside the sandbox and need a picker grant. */
const USER_PUBLIC_PATH_PREFIX: string = '/storage/Users/currentUser';
/** Only one directory is selected; the picker rejects a larger count. */
const MAX_SELECT_NUMBER: number = 1;
/**
 * Directory created under the picked root; the whole data root. Defined here
 * only: Rust receives the resolved path and never needs to know the name.
 */
const DATA_DIR_NAME: string = 'HiCodeer';

// Layout: one column, one left edge, matching the onboarding prototype.
const CONTENT_MAX_WIDTH: number = 552;
const PAGE_PADDING: number = 24;
const PAGE_TOP_PADDING: number = 56;
const PAGE_BOTTOM_PADDING: number = 40;
const LOGO_SIZE: number = 56;
const LOGO_RADIUS: number = 14;
const BRAND_GAP: number = 12;
/** Matches the logo box so the wordmark reads at the same size as the icon. */
const BRAND_TEXT_SIZE: number = LOGO_SIZE;
const BRAND_GAP_BOTTOM: number = 40;
const TITLE_SIZE: number = 28;
const TITLE_GAP_BOTTOM: number = 12;
const BODY_SIZE: number = 14;
const BODY_LINE_HEIGHT: number = 23;
const LEDE_GAP_BOTTOM: number = 26;
const LABEL_SIZE: number = 13;
const LABEL_GAP_BOTTOM: number = 10;
const CARD_PADDING: number = 16;
const CARD_RADIUS: number = 10;
const FIELD_HEIGHT: number = 46;
const FIELD_PADDING: number = 12;
const FIELD_RADIUS: number = 8;
const PATH_SIZE: number = 14;
const PATH_LINE_HEIGHT: number = 22;
const ACTION_SIZE: number = 14;
const ACTION_GAP_LEFT: number = 8;
const FINAL_GAP_TOP: number = 11;
const FINAL_LABEL_GAP_BOTTOM: number = 4;
const MAX_FINAL_PATH_LINES: number = 3;
const BUTTON_HEIGHT: number = 44;
const BUTTON_RADIUS: number = 8;
const BUTTON_GAP_TOP: number = 26;
const BUTTON_TEXT_SIZE: number = 14;
const BORDER_WIDTH: number = 1;
/** Kept short: the field should acknowledge the pointer without feeling slow. */
const HOVER_DURATION: number = 120;

let pendingResolve: ((homeDirectory: string) => void) | null = null;

/** Resolves with the chosen home directory once the user picks a folder. */
export function awaitSelection(): Promise<string> {
  return new Promise<string>((resolve: (homeDirectory: string) => void): void => {
    pendingResolve = resolve;
  });
}

/** Reads, records and resolves the chosen home directory (`<picked root>/.HiCodeer`). */
export class HomeDirectory {
  /** Absolute path of the record file inside the HAP private files dir. */
  static recordPath(context: common.UIAbilityContext): string {
    return `${context.filesDir}/${RECORD_FILE}`;
  }

  /** The home directory recorded by a previous launch, or '' when none is recorded. */
  static read(context: common.UIAbilityContext): string {
    const path = HomeDirectory.recordPath(context);
    try {
      if (!fs.accessSync(path)) {
        return '';
      }
      return fs.readTextSync(path).trim();
    } catch (error) {
      hilog.error(DOMAIN, TAG, 'read %{public}s failed: %{public}s', path, String(error));
      return '';
    }
  }

  /** Records the home directory so the next launch finds it. Idempotent. */
  static write(context: common.UIAbilityContext, homeDirectory: string): void {
    const path = HomeDirectory.recordPath(context);
    try {
      const file = fs.openSync(path, fs.OpenMode.READ_WRITE | fs.OpenMode.CREATE | fs.OpenMode.TRUNC);
      fs.writeSync(file.fd, homeDirectory);
      fs.closeSync(file);
    } catch (error) {
      hilog.error(DOMAIN, TAG, 'write %{public}s failed: %{public}s', path, String(error));
    }
  }

  /**
   * Returns the recorded `<root>/HiCodeer` when it is still usable, else ''.
   * Re-activates the persisted picker grant first, mirroring the Rust side.
   */
  static async resolveExisting(context: common.UIAbilityContext): Promise<string> {
    const home = HomeDirectory.read(context);
    if (home === '') {
      return '';
    }
    if (!home.startsWith(USER_PUBLIC_PATH_PREFIX)) {
      hilog.warn(DOMAIN, TAG, 'home directory outside user-public prefix: %{public}s', home);
      return home;
    }
    try {
      const uri = fileUri.getUriFromPath(home);
      await fileShare.activatePermission([{
        uri: uri,
        operationMode: fileShare.OperationMode.READ_MODE | fileShare.OperationMode.WRITE_MODE,
      }]);
    } catch (error) {
      hilog.warn(DOMAIN, TAG, 'activatePermission failed: %{public}s', String(error));
    }
    // statSync throws when the path is gone, so a directory the user deleted is
    // rejected here and the setup page is shown again instead of being reused.
    try {
      const info = fs.statSync(home);
      if (!info.isDirectory()) {
        hilog.warn(DOMAIN, TAG, 'home directory is not a directory: %{public}s', home);
        return '';
      }
      return home;
    } catch (error) {
      hilog.warn(DOMAIN, TAG, 'home directory not accessible: %{public}s', String(error));
      return '';
    }
  }

  /**
   * Creates `<root>/HiCodeer`, persists the grant, records it, and returns it.
   *
   * Returns '' when the directory cannot be created. The record is only written
   * once the directory exists: Rust uses the recorded path as the data root
   * without creating or checking it, so recording a missing directory would turn
   * into a failed launch instead of a page the user can retry on.
   *
   * The data directory name lives here only: Rust receives this resolved path
   * and treats it as the data root as-is.
   */
  static accept(root: string, uri: string, context: common.UIAbilityContext): string {
    const dataDirectory = `${root}/${DATA_DIR_NAME}`;
    if (!HomeDirectory.ensureDirectory(dataDirectory)) {
      return '';
    }
    try {
      fileShare.persistPermission([{
        uri: uri,
        operationMode: fileShare.OperationMode.READ_MODE | fileShare.OperationMode.WRITE_MODE,
      }]);
    } catch (error) {
      hilog.error(DOMAIN, TAG, 'persistPermission failed: %{public}s', String(error));
    }
    HomeDirectory.write(context, dataDirectory);
    return dataDirectory;
  }

  /**
   * Makes sure the data directory exists, reusing one an earlier install left.
   *
   * Existence is checked before creating rather than relying on recursive
   * mkdir to tolerate it: a leftover directory from a previous install is the
   * normal case here (re-picking the same root, or reinstalling), and it must
   * never count as a failure.
   */
  private static ensureDirectory(path: string): boolean {
    try {
      if (fs.statSync(path).isDirectory()) {
        return true;
      }
      hilog.error(DOMAIN, TAG, 'data path exists but is not a directory: %{public}s', path);
      return false;
    } catch (error) {
      // Missing, which is the expected first-run case; create it below.
    }
    try {
      fs.mkdirSync(path, true);
      return true;
    } catch (error) {
      hilog.error(DOMAIN, TAG, 'mkdir %{public}s: %{public}s', path, String(error));
      return false;
    }
  }
}

@Entry({ routeName: RouteName })
@Component
struct Setup {
  @State private selectedRoot: string = '';
  @State private finalPath: string = '';
  @State private preparing: boolean = false;
  /** Pointer is over the folder field, which is the control that opens the picker. */
  @State private pickHover: boolean = false;

  private async pick(): Promise<void> {
    const context = getContext(this) as common.UIAbilityContext;
    const selectOptions = new picker.DocumentSelectOptions();
    selectOptions.maxSelectNumber = MAX_SELECT_NUMBER;
    // Folder mode: the home directory is a directory, not a file.
    selectOptions.selectMode = picker.DocumentSelectMode.FOLDER;
    const documentPicker = new picker.DocumentViewPicker(context);
    let uris: string[] = [];
    try {
      uris = await documentPicker.select(selectOptions);
    } catch (error) {
      // A cancelled picker rejects or resolves empty; stay on this page either way.
      hilog.warn(DOMAIN, TAG, 'directory picker returned no selection: %{public}s', String(error));
      return;
    }
    if (uris.length === 0) {
      return;
    }
    const uri = uris[0];
    const root = new fileUri.FileUri(uri).path ?? '';
    if (root === '') {
      hilog.error(DOMAIN, TAG, 'cannot resolve a local path from uri %{public}s', uri);
      return;
    }
    const homeDirectory = HomeDirectory.accept(root, uri, context);
    this.selectedRoot = root;
    this.finalPath = homeDirectory;
  }

  /** Releases the native load gate; EntryAbility then starts the editor. */
  private next(): void {
    if (this.finalPath === '' || this.preparing) {
      return;
    }
    this.preparing = true;
    const resolve = pendingResolve;
    pendingResolve = null;
    if (resolve !== null) {
      resolve(this.finalPath);
    }
  }

  build() {
    Scroll() {
      Row() {
        Column() {
          Row({ space: BRAND_GAP }) {
            Image($r('app.media.app_icon'))
              .width(LOGO_SIZE)
              .height(LOGO_SIZE)
              .borderRadius(LOGO_RADIUS)
            Text('HiCodeer')
              .fontSize(BRAND_TEXT_SIZE)
              .fontWeight(FontWeight.Medium)
              .fontColor($r('app.color.setup_text_primary'))
          }
          .width('100%')
          .justifyContent(FlexAlign.Center)
          .margin({ bottom: BRAND_GAP_BOTTOM })

          Text($r('app.string.setup_title'))
            .width('100%')
            .fontSize(TITLE_SIZE)
            .fontWeight(FontWeight.Medium)
            .fontColor($r('app.color.setup_text_primary'))
            .margin({ bottom: TITLE_GAP_BOTTOM })

          Text($r('app.string.setup_lede'))
            .width('100%')
            .fontSize(BODY_SIZE)
            .lineHeight(BODY_LINE_HEIGHT)
            .fontColor($r('app.color.setup_text_secondary'))
            .margin({ bottom: LEDE_GAP_BOTTOM })

          Column() {
            Text($r('app.string.setup_data_folder'))
              .width('100%')
              .fontSize(LABEL_SIZE)
              .fontWeight(FontWeight.Medium)
              .fontColor($r('app.color.setup_text_tertiary'))
              .margin({ bottom: LABEL_GAP_BOTTOM })

            Row() {
              if (this.selectedRoot === '') {
                Text($r('app.string.setup_no_folder'))
                  .fontSize(PATH_SIZE)
                  .fontColor($r('app.color.setup_text_tertiary'))
                  .maxLines(1)
                  .textOverflow({ overflow: TextOverflow.Ellipsis })
                  .layoutWeight(1)
              } else {
                Text(this.selectedRoot)
                  .fontSize(PATH_SIZE)
                  .fontColor($r('app.color.setup_text_primary'))
                  .maxLines(1)
                  .textOverflow({ overflow: TextOverflow.Ellipsis })
                  .layoutWeight(1)
              }

              if (this.selectedRoot === '') {
                Text($r('app.string.setup_choose'))
                  .fontSize(ACTION_SIZE)
                  .fontWeight(FontWeight.Medium)
                  .fontColor($r('app.color.setup_accent'))
                  .margin({ left: ACTION_GAP_LEFT })
              } else {
                Text($r('app.string.setup_change'))
                  .fontSize(ACTION_SIZE)
                  .fontWeight(FontWeight.Medium)
                  .fontColor($r('app.color.setup_accent'))
                  .margin({ left: ACTION_GAP_LEFT })
              }
            }
            .width('100%')
            .height(FIELD_HEIGHT)
            .padding({ left: FIELD_PADDING, right: FIELD_PADDING })
            .borderRadius(FIELD_RADIUS)
            .backgroundColor(this.pickHover
              ? $r('app.color.setup_field_hover')
              : $r('app.color.setup_field'))
            .border({
              width: BORDER_WIDTH,
              color: this.pickHover
                ? $r('app.color.setup_border_hover')
                : $r('app.color.setup_border'),
              style: this.selectedRoot === '' ? BorderStyle.Dashed : BorderStyle.Solid,
            })
            .animation({ duration: HOVER_DURATION, curve: Curve.EaseOut })
            .hoverEffect(HoverEffect.None)
            .onHover((isHover: boolean): void => {
              this.pickHover = isHover;
            })
            .onClick((): void => {
              this.pick();
            })

            if (this.finalPath !== '') {
              Column() {
                Text($r('app.string.setup_final_path'))
                  .width('100%')
                  .fontSize(LABEL_SIZE)
                  .fontWeight(FontWeight.Medium)
                  .fontColor($r('app.color.setup_text_tertiary'))
                  .margin({ bottom: FINAL_LABEL_GAP_BOTTOM })

                Text(this.finalPath)
                  .width('100%')
                  .fontSize(PATH_SIZE)
                  .lineHeight(PATH_LINE_HEIGHT)
                  .fontColor($r('app.color.setup_text_secondary'))
                  .maxLines(MAX_FINAL_PATH_LINES)
                  .textOverflow({ overflow: TextOverflow.Ellipsis })
              }
              .width('100%')
              .alignItems(HorizontalAlign.Start)
              .margin({ top: FINAL_GAP_TOP })
            }
          }
          .width('100%')
          .alignItems(HorizontalAlign.Start)
          .padding(CARD_PADDING)
          .borderRadius(CARD_RADIUS)
          .backgroundColor($r('app.color.setup_surface'))
          .border({
            width: BORDER_WIDTH,
            color: $r('app.color.setup_border'),
            style: BorderStyle.Solid,
          })

          Button() {
            Text(this.preparing ? $r('app.string.setup_preparing') : $r('app.string.setup_next'))
              .fontSize(BUTTON_TEXT_SIZE)
              .fontWeight(FontWeight.Medium)
              .fontColor(this.finalPath === ''
                ? $r('app.color.setup_text_tertiary')
                : $r('app.color.setup_on_accent'))
          }
          .width('100%')
          .height(BUTTON_HEIGHT)
          .borderRadius(BUTTON_RADIUS)
          .backgroundColor(this.finalPath === ''
            ? $r('app.color.setup_disabled')
            : $r('app.color.setup_accent'))
          .enabled(this.finalPath !== '' && !this.preparing)
          .margin({ top: BUTTON_GAP_TOP })
          .onClick((): void => {
            this.next();
          })
        }
        .width('100%')
        .constraintSize({ maxWidth: CONTENT_MAX_WIDTH })
        .padding({
          left: PAGE_PADDING,
          right: PAGE_PADDING,
          top: PAGE_TOP_PADDING,
          bottom: PAGE_BOTTOM_PADDING,
        })
        .alignItems(HorizontalAlign.Start)
      }
      .width('100%')
      .justifyContent(FlexAlign.Center)
      .alignItems(VerticalAlign.Top)
    }
    .width('100%')
    .height('100%')
    .scrollBar(BarState.Off)
    .backgroundColor($r('app.color.setup_background'))
  }
}
