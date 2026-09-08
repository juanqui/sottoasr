const repository = 'https://github.com/juanqui/sottoasr';
const endpoint = 'https://api.github.com/repos/juanqui/sottoasr/releases/latest';

export async function updateRelease(document, fetchRelease = fetch) {
    const version = document.querySelector('[data-release-version]');
    if (!version) return;

    try {
        const response = await fetchRelease(endpoint, {
            signal: AbortSignal.timeout(5000),
            credentials: 'omit',
            referrerPolicy: 'no-referrer',
            cache: 'no-cache',
            headers: { Accept: 'application/vnd.github+json' },
        });
        if (!response.ok) return;
        const release = await response.json();
        if (!release || release.draft !== false || release.prerelease !== false ||
            typeof release.tag_name !== 'string' || !/^v\d+\.\d+\.\d+$/.test(release.tag_name)) return;

        const filename = `SottoASR_${release.tag_name.slice(1)}_aarch64.dmg`;
        if (!Array.isArray(release.assets) || !release.assets.some(asset =>
            asset?.name === filename && asset.state === 'uploaded' && asset.size > 0)) return;

        version.querySelector('.version-badge').textContent = release.tag_name;
        for (const link of document.querySelectorAll('[data-release-link]')) {
            link.href = `${repository}/releases/tag/${release.tag_name}`;
        }
        version.hidden = false;
    } catch {
        // Keep the generic latest-release links usable when GitHub is unavailable.
    }
}

if (typeof document !== 'undefined') void updateRelease(document);
