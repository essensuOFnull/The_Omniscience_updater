import { appendFile, copyFile, mkdir, stat } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { createReadStream, createWriteStream } from 'node:fs';
import { Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const executable = path.join(projectRoot, 'src-tauri', 'target', 'release', 'omniscience-updater.exe');
const resourceArchive = path.join(projectRoot, 'src-tauri', 'resources', 'portable', 'updater.pack.gz');
const outputDirectory = path.join(projectRoot, 'dist');
const outputExecutable = path.join(outputDirectory, 'OmniscienceUpdater-Portable.exe');

await stat(executable);
const archiveInfo = await stat(resourceArchive);
await mkdir(outputDirectory, { recursive: true });
await copyFile(executable, outputExecutable);

const hash = createHash('sha256');
const hashStream = new Transform({
  transform(chunk, encoding, callback) {
    hash.update(chunk);
    callback(null, chunk);
  },
});
await pipeline(
  createReadStream(resourceArchive),
  hashStream,
  createWriteStream(outputExecutable, { flags: 'a' }),
);

const trailer = Buffer.alloc(48);
trailer.write('OMNIPRT1', 0, 'ascii');
trailer.writeBigUInt64LE(BigInt(archiveInfo.size), 8);
hash.digest().copy(trailer, 16);
await appendFile(outputExecutable, trailer);

const { size } = await stat(outputExecutable);
console.log(
  `Portable executable: ${path.relative(projectRoot, outputExecutable)} ` +
  `(${(size / 1024 / 1024).toFixed(1)} MiB; includes ${path.basename(resourceArchive)}).`,
);
