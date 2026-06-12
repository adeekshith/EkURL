// Scripts are loaded with `defer`, so the DOM is parsed before this runs.
const $ = (id) => document.getElementById(id);

const form = $('shorten-form');
const result = $('result');
const errorBox = $('error');
const shortenBtn = $('shorten-btn');
const copyBtn = $('copy-btn');
const shortLink = $('short-link');
const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'always' });

let shortUrl = '';

form.addEventListener('submit', onSubmit);
copyBtn.addEventListener('click', onCopy);
$('open-btn').addEventListener('click', () => window.open(shortUrl, '_blank', 'noopener'));
$('reset-btn').addEventListener('click', resetForm);

if (typeof navigator.share === 'function') {
    const shareBtn = $('share-btn');
    shareBtn.hidden = false;
    shareBtn.addEventListener('click', onShare);
}

$('url').focus();

async function onSubmit(event) {
    event.preventDefault();

    const url = $('url').value.trim();
    const custom_code = $('custom_code').value.trim();
    const expires_in = $('expires_in').value;

    errorBox.textContent = '';
    if (!url) return showError('URL is required');

    setLoading(true);
    try {
        const res = await fetch('/api/v1/shorten', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ url, custom_code: custom_code || null, expires_in }),
        });
        const data = await res.json();
        if (res.ok) showSuccess(data);
        else showError(data.error || 'Something went wrong');
    } catch {
        showError('Failed to connect to the server');
    } finally {
        setLoading(false);
    }
}

function showSuccess({ code, expires_at }) {
    shortUrl = `${location.origin}/${code}`;
    shortLink.href = shortUrl;
    shortLink.textContent = shortUrl;

    const expiryInfo = $('expiry-info');
    expiryInfo.textContent = relativeExpiry(expires_at);
    if (expires_at) expiryInfo.title = new Date(expires_at * 1000).toLocaleString();
    else expiryInfo.removeAttribute('title');

    renderQR(shortUrl);
    form.hidden = true;
    result.hidden = false;
    copyBtn.focus();
}

function relativeExpiry(expiresAt) {
    if (!expiresAt) return 'This link never expires';
    const secs = expiresAt - Math.floor(Date.now() / 1000);
    if (secs <= 0) return 'Expired';
    const [value, unit] =
        secs >= 31536000 ? [Math.round(secs / 31536000), 'year']
        : secs >= 2592000 ? [Math.round(secs / 2592000), 'month']
        : secs >= 86400 ? [Math.round(secs / 86400), 'day']
        : [Math.max(1, Math.round(secs / 3600)), 'hour'];
    return `Expires ${rtf.format(value, unit)}`;
}

function renderQR(text) {
    const canvas = $('qr-canvas');
    const ctx = canvas.getContext('2d');
    const size = canvas.width;

    ctx.fillStyle = '#fff';
    ctx.fillRect(0, 0, size, size);

    try {
        const qr = qrcode(0, 'M');
        qr.addData(text);
        qr.make();
        const count = qr.getModuleCount();
        const cell = Math.floor(size / (count + 2));
        const offset = Math.floor((size - cell * count) / 2);
        ctx.fillStyle = '#000';
        for (let r = 0; r < count; r++) {
            for (let c = 0; c < count; c++) {
                if (qr.isDark(r, c)) ctx.fillRect(offset + c * cell, offset + r * cell, cell, cell);
            }
        }
    } catch (err) {
        console.error('QR render failed:', err);
        ctx.fillStyle = '#94a3b8';
        ctx.font = '12px system-ui';
        ctx.textAlign = 'center';
        ctx.fillText('QR unavailable', size / 2, size / 2);
    }
}

async function onCopy() {
    try {
        await navigator.clipboard.writeText(shortUrl);
        const original = copyBtn.textContent;
        copyBtn.textContent = 'Copied!';
        setTimeout(() => { copyBtn.textContent = original; }, 2000);
    } catch {
        selectShortLink();
    }
}

function selectShortLink() {
    const range = document.createRange();
    range.selectNodeContents(shortLink);
    const sel = getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
}

async function onShare() {
    try {
        await navigator.share({ url: shortUrl, title: 'Short link' });
    } catch (err) {
        if (err?.name !== 'AbortError') console.error('Share failed:', err);
    }
}

function resetForm() {
    $('url').value = '';
    $('custom_code').value = '';
    errorBox.textContent = '';
    result.hidden = true;
    form.hidden = false;
    $('url').focus();
}

function showError(msg) {
    errorBox.textContent = msg;
}

function setLoading(on) {
    shortenBtn.disabled = on;
    shortenBtn.textContent = on ? 'Shortening...' : 'Shorten URL';
}
