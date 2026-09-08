import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { JSDOM } from 'jsdom';
import { updateRelease } from '../website/release.js';

const html = readFileSync(new URL('../website/index.html', import.meta.url), 'utf8');
const latest = 'https://github.com/juanqui/sottoasr/releases/latest';
const published = {
    tag_name: 'v99.1.2', draft: false, prerelease: false,
    assets: [{ name: 'SottoASR_99.1.2_aarch64.dmg', state: 'uploaded', size: 100 }],
};
const document = () => new JSDOM(html).window.document;
const fallback = (page) => {
    assert.equal(page.querySelector('[data-release-version]').hidden, true);
    const links = [...page.querySelectorAll('[data-release-link]')];
    assert.equal(links.length, 2);
    assert.deepEqual(links.map(link => link.href), [latest, latest]);
};

test('HTML has usable latest links and hides the unverified source version', () => {
    const page = document();
    fallback(page);
    assert.equal(page.querySelector('script[type="module"]').getAttribute('src'), 'release.js');
});

test('published release updates the visible badge and both links together', async () => {
    const page = document();
    await updateRelease(page, async (url, options) => {
        assert.equal(url, 'https://api.github.com/repos/juanqui/sottoasr/releases/latest');
        assert.equal(options.credentials, 'omit');
        assert.equal(options.referrerPolicy, 'no-referrer');
        assert.equal(options.cache, 'no-cache');
        assert.ok(options.signal instanceof AbortSignal);
        return { ok: true, json: async () => published };
    });
    assert.equal(page.querySelector('[data-release-version]').hidden, false);
    assert.equal(page.querySelector('.version-badge').textContent, published.tag_name);
    assert.deepEqual([...page.querySelectorAll('[data-release-link]')].map(link => link.href),
        Array(2).fill('https://github.com/juanqui/sottoasr/releases/tag/v99.1.2'));
});

test('draft, prerelease, malformed and incomplete releases retain the fallback', async () => {
    for (const release of [null, {}, { ...published, draft: true },
        { ...published, prerelease: true }, { ...published, tag_name: '<script>' },
        { ...published, assets: [] }, { ...published, assets: [null] },
        { ...published, assets: [{ ...published.assets[0], state: 'new' }] },
        { ...published, assets: [{ ...published.assets[0], size: 0 }] }]) {
        const page = document();
        await updateRelease(page, async () => ({ ok: true, json: async () => release }));
        fallback(page);
    }
});

test('HTTP, JSON and network failures preserve usable download links', async () => {
    for (const fetchRelease of [async () => ({ ok: false }),
        async () => { throw new Error('offline or timed out'); },
        async () => ({ ok: true, json: async () => { throw new Error('invalid JSON'); } })]) {
        const page = document();
        await updateRelease(page, fetchRelease);
        fallback(page);
    }
});
