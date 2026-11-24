import fs from 'fs/promises';
import fsSync from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ORIGINALS_DIR = path.join(__dirname, 'originals');
const FRAGMENTS_DIR = path.join(__dirname, 'fragments');
const CHUNK_SIZE = 50 * 1024 * 1024; // 50 МБ

// Цвета для консоли
const c = {
  reset: '\x1b[0m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  cyan: '\x1b[36m',
  red: '\x1b[31m'
};

async function ensureDirs() {
  await fs.mkdir(ORIGINALS_DIR, { recursive: true });
  await fs.mkdir(FRAGMENTS_DIR, { recursive: true });
}

// --- РАЗДЕЛЕНИЕ (Originals -> Fragments) ---
async function splitFile(filename) {
  const origPath = path.join(ORIGINALS_DIR, filename);
  const stat = await fs.stat(origPath);
  const fileSize = stat.size;

  // Проверяем, существуют ли уже фрагменты и совпадает ли их общий размер
  let fragSize = 0;
  let part = 0;
  while (true) {
    const fragPath = path.join(FRAGMENTS_DIR, `${filename}.${part}`);
    if (!fsSync.existsSync(fragPath)) break;
    fragSize += (await fs.stat(fragPath)).size;
    part++;
  }

  if (fragSize === fileSize && part > 0) {
    return false; // Всё уже разбито
  }

  console.log(`${c.yellow}⚡ Разбиваю:${c.reset} ${filename} (${(fileSize / 1024 / 1024).toFixed(2)} MB)`);

  // Удаляем старые фрагменты, если они были (на случай изменения файла)
  part = 0;
  while (true) {
    const fragPath = path.join(FRAGMENTS_DIR, `${filename}.${part}`);
    if (!fsSync.existsSync(fragPath)) break;
    await fs.unlink(fragPath);
    part++;
  }

  // Читаем и пишем по 50 МБ через потоки
  const fd = await fs.open(origPath, 'r');
  let bytesRead = 0;
  part = 0;

  while (bytesRead < fileSize) {
    const fragPath = path.join(FRAGMENTS_DIR, `${filename}.${part}`);
    const writeStream = fsSync.createWriteStream(fragPath);
    
    const end = Math.min(bytesRead + CHUNK_SIZE, fileSize);
    const readStream = fsSync.createReadStream(origPath, { start: bytesRead, end: end - 1 });

    await new Promise((resolve, reject) => {
      readStream.on('error', reject);
      writeStream.on('error', reject);
      writeStream.on('finish', resolve);
      readStream.pipe(writeStream);
    });

    bytesRead = end;
    part++;
  }
  await fd.close();
  return true;
}

// --- СБОРКА (Fragments -> Originals) ---
async function joinFile(baseName, fragmentsCount) {
  const origPath = path.join(ORIGINALS_DIR, baseName);
  
  // Считаем общий размер фрагментов
  let totalSize = 0;
  for (let i = 0; i < fragmentsCount; i++) {
    const fragPath = path.join(FRAGMENTS_DIR, `${baseName}.${i}`);
    totalSize += (await fs.stat(fragPath)).size;
  }

  // Если файл уже существует и его размер совпадает — пропускаем
  if (fsSync.existsSync(origPath)) {
    const stat = await fs.stat(origPath);
    if (stat.size === totalSize) {
      return false;
    }
  }

  console.log(`${c.cyan}🔧 Собираю:${c.reset} ${baseName} из ${fragmentsCount} частей (${(totalSize / 1024 / 1024).toFixed(2)} MB)`);

  const writeStream = fsSync.createWriteStream(origPath);

  for (let i = 0; i < fragmentsCount; i++) {
    const fragPath = path.join(FRAGMENTS_DIR, `${baseName}.${i}`);
    const readStream = fsSync.createReadStream(fragPath);

    await new Promise((resolve, reject) => {
      readStream.on('error', reject);
      writeStream.on('error', reject);
      readStream.on('end', resolve);
      readStream.pipe(writeStream, { end: false });
    });
  }

  await new Promise((resolve) => writeStream.end(resolve));
  return true;
}

async function main() {
  await ensureDirs();

  // ШАГ 1: Разбиваем всё, что есть в originals
  const origFiles = await fs.readdir(ORIGINALS_DIR);
  let splitCount = 0;
  for (const file of origFiles) {
    const stat = await fs.stat(path.join(ORIGINALS_DIR, file));
    if (stat.isFile()) {
      const didSplit = await splitFile(file);
      if (didSplit) splitCount++;
    }
  }

  // ШАГ 2: Собираем всё, что есть в fragments, если нет в originals
  const fragFiles = await fs.readdir(FRAGMENTS_DIR);
  const bases = new Map(); // baseName -> count

  for (const file of fragFiles) {
    const match = file.match(/^(.*)\.(\d+)$/);
    if (match) {
      const baseName = match[1];
      const partNum = parseInt(match[2], 10);
      const currentMax = bases.get(baseName) || 0;
      if (partNum >= currentMax) {
        bases.set(baseName, partNum + 1);
      }
    }
  }

  let joinCount = 0;
  for (const [baseName, count] of bases.entries()) {
    const didJoin = await joinFile(baseName, count);
    if (didJoin) joinCount++;
  }

  // Итоги
  console.log(`\n${c.green}✅ Готово!${c.reset}`);
  if (splitCount === 0 && joinCount === 0) {
    console.log(`Все файлы уже синхронизированы.`);
  } else {
    console.log(`Разбито на фрагменты: ${splitCount}`);
    console.log(`Собрано из фрагментов: ${joinCount}`);
  }
}

main().catch((err) => {
  console.error(`${c.red}❌ Ошибка:${c.reset}`, err);
  process.exit(1);
});