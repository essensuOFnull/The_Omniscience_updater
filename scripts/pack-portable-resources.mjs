import { createReadStream, createWriteStream } from 'node:fs';
import { mkdir, readdir, rm, stat } from 'node:fs/promises';
import { createGzip, constants as zlibConstants } from 'node:zlib';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(scriptDirectory, '..');
const resourcesRoot = path.join(projectRoot, 'src-tauri', 'resources', 'generated', 'updater');
const portableRoot = path.join(projectRoot, 'src-tauri', 'resources', 'portable');
const archivePath = path.join(portableRoot, 'updater.pack.gz');

async function collectFiles(directory, relativeDirectory = '') {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const relativePath = path.join(relativeDirectory, entry.name);
    const absolutePath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await collectFiles(absolutePath, relativePath));
    } else if (entry.isFile()) {
      files.push({ absolutePath, relativePath });
    } else {
      throw new Error(`Unsupported resource entry: ${absolutePath}`);
    }
  }
  return files;
}

const files = (await collectFiles(resourcesRoot))
  .sort((left, right) => left.relativePath.localeCompare(right.relativePath));
if (files.length === 0 || files.length > 0xffffffff) {
  throw new Error(`Invalid portable resource count: ${files.length}`);
}

const entries = [];
let uncompressedBytes = 0;
for (const file of files) {
  const relativePath = file.relativePath.split(path.sep).join('/');
  const pathBytes = Buffer.from(relativePath, 'utf8');
  if (pathBytes.length === 0 || pathBytes.length > 0xffff) {
    throw new Error(`Invalid portable resource path: ${relativePath}`);
  }
  const fileInfo = await stat(file.absolutePath);
  entries.push({ ...file, pathBytes, size: fileInfo.size });
  uncompressedBytes += fileInfo.size;
}

async function* archiveContent() {
  const header = Buffer.alloc(12);
  header.write('OMNIPK01', 0, 'ascii');
  header.writeUInt32LE(entries.length, 8);
  yield header;

  for (const entry of entries) {
    const entryHeader = Buffer.alloc(10);
    entryHeader.writeUInt16LE(entry.pathBytes.length, 0);
    entryHeader.writeBigUInt64LE(BigInt(entry.size), 2);
    yield entryHeader;
    yield entry.pathBytes;
    for await (const chunk of createReadStream(entry.absolutePath)) {
      yield chunk;
    }
  }
}

await mkdir(portableRoot, { recursive: true });
await rm(archivePath, { force: true });
try {
  await pipeline(
    Readable.from(archiveContent()),
    createGzip({ level: zlibConstants.Z_BEST_SPEED }),
    createWriteStream(archivePath),
  );
} catch (error) {
  await rm(archivePath, { force: true });
  throw error;
}

const archiveBytes = (await stat(archivePath)).size;
console.log(
  `Packed ${entries.length} files (${(uncompressedBytes / 1024 / 1024).toFixed(1)} MiB) into ` +
  `${path.relative(projectRoot, archivePath)} (${(archiveBytes / 1024 / 1024).toFixed(1)} MiB).`,
);
