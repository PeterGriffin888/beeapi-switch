const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const root = path.resolve(__dirname, '..');
const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const releaseDir = path.join(root, 'release');

function run(cmd, args, options = {}) {
  console.log(`> ${cmd} ${args.join(' ')}`);
  execFileSync(cmd, args, { stdio: 'inherit', cwd: root, shell: process.platform === 'win32', ...options });
}

fs.mkdirSync(releaseDir, { recursive: true });
run('npm', ['run', 'tauri', 'build', '--', '--bundles', 'nsis']);

const setupName = `BeeAPI Switch_${pkg.version}_x64-setup.exe`;
const src = path.join(root, 'src-tauri', 'target', 'release', 'bundle', 'nsis', setupName);
const stamp = new Date().toISOString().slice(0, 10).replace(/-/g, '');
const dstLatest = path.join(releaseDir, 'BeeAPI-Switch-windows-x64-latest.exe');
const dstVersioned = path.join(releaseDir, `BeeAPI-Switch-${pkg.version}-${stamp}-windows-x64.exe`);

if (!fs.existsSync(src)) {
  throw new Error(`NSIS installer not found: ${src}`);
}
fs.copyFileSync(src, dstLatest);
fs.copyFileSync(src, dstVersioned);

const appExe = path.join(root, 'src-tauri', 'target', 'release', 'beeapi-switch.exe');
if (fs.existsSync(appExe)) {
  fs.copyFileSync(appExe, path.join(releaseDir, 'beeapi-switch.exe'));
}

console.log('\nRelease artifacts:');
for (const file of [dstLatest, dstVersioned, path.join(releaseDir, 'beeapi-switch.exe')]) {
  if (fs.existsSync(file)) {
    const mb = (fs.statSync(file).size / 1024 / 1024).toFixed(2);
    console.log(`- ${file} (${mb} MB)`);
  }
}
