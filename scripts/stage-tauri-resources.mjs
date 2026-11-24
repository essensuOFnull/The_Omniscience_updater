import { cp, mkdir, readdir, rm } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(scriptDirectory, '..');
const generatedRoot = path.join(projectRoot, 'src-tauri', 'resources', 'generated', 'updater');
const fragmentsSource = path.join(projectRoot, 'fragmenter', 'fragments');
const fragmentsDestination = path.join(generatedRoot, 'fragmenter', 'fragments');
const isCheck = process.argv.includes('--check');

const fragmentBases = [
  'Microsoft.DesktopAppInstaller_8wekyb3d8bbwe.msixbundle',
  'Microsoft.VCLibs.140.00.UWPDesktop_14.0.33728.0_x64.appx',
  'Microsoft.UI.Xaml.2.8_8.2501.31001.0_x64__8wekyb3d8bbwe.Appx',
  'Microsoft.NET.Native.Framework.2.2_2.2.29512.0_x64__8wekyb3d8bbwe.Appx',
  'Microsoft.NET.Native.Runtime.2.2_2.2.28604.0_x64__8wekyb3d8bbwe.Appx',
  'Microsoft.WindowsStore_22610.1401.8.0_neutral_~_8wekyb3d8bbwe.Msixbundle',
  'Microsoft.NET.Native.Framework.1.7_1.7.27413.0_x64__8wekyb3d8bbwe.Appx',
  'Microsoft.NET.Native.Runtime.1.7.appx',
  'Microsoft.UI.Xaml.2.4_2.42007.9001.0_x64__8wekyb3d8bbwe.appx',
  'Microsoft.VCLibs.140.00_14.0.33519.0_x64.appx',
];

const expectedFragments = [];
for (const base of fragmentBases) {
  const files = await readdir(fragmentsSource);
  const parts = files
    .map((name) => {
      const match = name.match(new RegExp(`^${base.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\.(\\d+)$`));
      return match ? Number(match[1]) : null;
    })
    .filter((part) => part !== null)
    .sort((left, right) => left - right);

  if (parts.length === 0 || parts.some((part, index) => part !== index)) {
    throw new Error(`Missing or discontinuous fragment set for ${base}`);
  }
  expectedFragments.push(...parts.map((part) => `${base}.${part}`));
}

const ptyPackages = [
  path.join(projectRoot, 'node_modules', '@lydell', 'node-pty'),
  path.join(projectRoot, 'node_modules', '@lydell', 'node-pty-win32-x64'),
];
for (const packagePath of ptyPackages) {
  await readdir(packagePath);
}

if (isCheck) {
  console.log(`Resource staging inputs are available (${expectedFragments.length} payload fragments).`);
  process.exit(0);
}

await rm(generatedRoot, { recursive: true, force: true });
await mkdir(path.join(generatedRoot, 'scripts'), { recursive: true });
await mkdir(fragmentsDestination, { recursive: true });
await mkdir(path.join(generatedRoot, 'node_modules', '@lydell'), { recursive: true });

await cp(process.execPath, path.join(generatedRoot, 'node.exe'));
await cp(path.join(scriptDirectory, 'pty-host.cjs'), path.join(generatedRoot, 'pty-host.cjs'));
for (const scriptName of ['Restore-Payload.ps1', 'Install-Winget.ps1', 'Install-Store.ps1']) {
  await cp(path.join(scriptDirectory, scriptName), path.join(generatedRoot, 'scripts', scriptName));
}

for (const packagePath of ptyPackages) {
  const destination = path.join(generatedRoot, 'node_modules', '@lydell', path.basename(packagePath));
  await cp(packagePath, destination, {
    recursive: true,
    filter: (source) => !source.toLowerCase().endsWith('.pdb'),
  });
}

for (const fragment of expectedFragments) {
  await cp(path.join(fragmentsSource, fragment), path.join(fragmentsDestination, fragment));
}

console.log(`Staged bundled Node, node-pty and ${expectedFragments.length} payload fragments.`);
