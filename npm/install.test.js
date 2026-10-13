const { test } = require('node:test');
const assert = require('node:assert/strict');
const { getPlatformInfo, downloadArchive } = require('./install');

const info = {
  extension: '.tar.gz', filename: 'crabcode-test.tar.gz',
  url: 'https://example.com/crabcode-test.tar.gz',
};

test('current platform uses registry-compatible archives', () => {
  assert.equal(getPlatformInfo().extension, process.platform === 'win32' ? '.zip' : '.tar.gz');
});

test('gzip download succeeds without fallback', async () => {
  const urls = [];
  assert.equal(await downloadArchive(info, '/unused', async (url) => urls.push(url)), info);
  assert.deepEqual(urls, [info.url]);
});

test('older xz assets are retried only after gzip 404', async () => {
  const urls = [];
  const result = await downloadArchive(info, '/unused', async (url) => {
    urls.push(url);
    if (urls.length === 1) throw Object.assign(new Error('missing'), { statusCode: 404 });
  });
  assert.equal(result.extension, '.tar.xz');
  assert.deepEqual(urls, [info.url, 'https://example.com/crabcode-test.tar.xz']);
});

test('network failures and Windows zip failures do not fallback', async () => {
  for (const [asset, error] of [[info, new Error('network')], [{ ...info, extension: '.zip' }, Object.assign(new Error('missing'), { statusCode: 404 })]]) {
    let attempts = 0;
    await assert.rejects(downloadArchive(asset, '/unused', async () => { attempts++; throw error; }), error);
    assert.equal(attempts, 1);
  }
});
