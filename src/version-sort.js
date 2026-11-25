const naturalCollator = new Intl.Collator('en', { numeric: true, sensitivity: 'base' });

function parseVersionTag(tag) {
  const match = String(tag).match(
    /^v?(\d+(?:\.\d+)*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/,
  );
  if (!match) return null;

  return {
    numbers: match[1].split('.').map(Number),
    prerelease: match[2] ? match[2].split('.') : null,
  };
}

function comparePrerelease(left, right) {
  if (left === null && right === null) return 0;
  if (left === null) return 1;
  if (right === null) return -1;

  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    if (left[index] === undefined) return -1;
    if (right[index] === undefined) return 1;
    const leftIsNumeric = /^\d+$/.test(left[index]);
    const rightIsNumeric = /^\d+$/.test(right[index]);
    if (leftIsNumeric && rightIsNumeric) {
      const difference = Number(left[index]) - Number(right[index]);
      if (difference) return Math.sign(difference);
    } else if (leftIsNumeric !== rightIsNumeric) {
      return leftIsNumeric ? -1 : 1;
    } else {
      const difference = naturalCollator.compare(left[index], right[index]);
      if (difference) return Math.sign(difference);
    }
  }
  return 0;
}

export function compareReleaseTags(leftTag, rightTag) {
  const left = parseVersionTag(leftTag);
  const right = parseVersionTag(rightTag);
  if (!left || !right) return naturalCollator.compare(leftTag, rightTag);

  for (let index = 0; index < Math.max(left.numbers.length, right.numbers.length); index += 1) {
    const difference = (left.numbers[index] || 0) - (right.numbers[index] || 0);
    if (difference) return Math.sign(difference);
  }

  return comparePrerelease(left.prerelease, right.prerelease);
}
