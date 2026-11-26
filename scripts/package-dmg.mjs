// Run on macOS after `tauri build --target universal-apple-darwin --bundles app`.
// Tauri's DMG schema has no extra-files option; use the same layout settings here.
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { resolve } from 'node:path';

const run = (command, args) => execFileSync(command, args, { stdio: 'inherit' });
const layout = JSON.parse(readFileSync('apps/desktop/src-tauri/tauri.macos.conf.json')).bundle.macOS.dmg;
const stage = resolve('target/dmg-stage');
const mount = resolve('target/dmg-mount');
const image = resolve('target/MobBoss-layout.dmg');
const output = resolve('target/MobBoss-macOS.dmg');
rmSync(stage, { recursive: true, force: true });
mkdirSync(stage, { recursive: true });
mkdirSync(mount, { recursive: true });
run('ditto', ['target/universal-apple-darwin/release/bundle/macos/Mob Boss.app', `${stage}/Mob Boss.app`]);
symlinkSync('/Applications', `${stage}/Applications`);
cpSync('docs/packaging/macos/How to open Mob Boss.txt', `${stage}/How to open Mob Boss.txt`);
run('hdiutil', ['create', '-ov', '-volname', 'Mob Boss', '-srcfolder', stage, '-format', 'UDRW', image]);
run('hdiutil', ['attach', '-nobrowse', '-mountpoint', mount, image]);
try {
  const { width, height } = layout.windowSize;
  const app = layout.appPosition;
  const applications = layout.applicationFolderPosition;
  // App and Applications retain the standard Tauri positions. The note is on a
  // separate centered row, with space for its full filename below the icon.
  run('osascript', ['-e', `tell application "Finder"
    tell disk "Mob Boss"
      open
      set current view of container window to icon view
      set toolbar visible of container window to false
      set statusbar visible of container window to false
      set bounds of container window to {100, 100, ${100 + width}, ${100 + height}}
      set arrangement of icon view options of container window to not arranged
      set icon size of icon view options of container window to 80
      set position of item "Mob Boss.app" to {${app.x}, ${app.y}}
      set position of item "Applications" to {${applications.x}, ${applications.y}}
      set position of item "How to open Mob Boss.txt" to {330, 330}
      close
      open
      update without registering applications
      delay 2
      close
    end tell
  end tell`]);
} finally {
  run('hdiutil', ['detach', mount]);
}
run('hdiutil', ['convert', image, '-format', 'UDZO', '-ov', '-o', output]);
run('hdiutil', ['verify', output]);
