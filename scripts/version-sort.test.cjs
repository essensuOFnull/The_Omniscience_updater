const test = require('node:test');
const assert = require('node:assert/strict');

test('orders semantic release tags newest first, with stable releases before prereleases', async () => {
  const { compareReleaseTags } = await import('../src/version-sort.js');
  const tags = [
    'v13.9.9',
    'v13.10.0-beta.2',
    'v13.10.0-beta.11',
    'v13.10.0',
    'v13.2.0',
  ];

  tags.sort((left, right) => compareReleaseTags(right, left));

  assert.deepEqual(tags, [
    'v13.10.0',
    'v13.10.0-beta.11',
    'v13.10.0-beta.2',
    'v13.9.9',
    'v13.2.0',
  ]);
});
