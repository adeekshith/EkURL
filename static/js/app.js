let currentShortUrl = '';

document.addEventListener('DOMContentLoaded', () => {
    document.getElementById('shorten-form').addEventListener('submit', onSubmit);
    document.getElementById('copy-btn').addEventListener('click', onCopy);
    document.getElementById('open-btn').addEventListener('click', onOpen);
    document.getElementById('share-btn').addEventListener('click', onShare);
    document.getElementById('reset-btn').addEventListener('click', resetForm);

    if (typeof navigator.share === 'function') {
        document.getElementById('share-btn').hidden = false;
    }

    document.getElementById('url').focus();
});

async function onSubmit(event) {
    event.preventDefault();

    const url = document.getElementById('url').value.trim();
    const custom_code = document.getElementById('custom_code').value.trim();
    const expires_in = document.getElementById('expires_in').value;
    const errorDiv = document.getElementById('error');

    errorDiv.textContent = '';

    if (!url) {
        showError('URL is required');
        return;
    }

    setLoading(true);

    try {
        const response = await fetch('/api/v1/shorten', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ url, custom_code: custom_code || null, expires_in })
        });

        const data = await response.json();

        if (response.ok) {
            showSuccess(data);
        } else {
            showError(data.error || 'Something went wrong');
        }
    } catch (err) {
        showError('Failed to connect to the server');
    } finally {
        setLoading(false);
    }
}

function showSuccess({ code, expires_at }) {
    currentShortUrl = window.location.origin + '/' + code;

    const link = document.getElementById('short-link');
    link.href = currentShortUrl;
    link.textContent = currentShortUrl;

    const expiryInfo = document.getElementById('expiry-info');
    expiryInfo.textContent = relativeExpiry(expires_at);
    if (expires_at) {
        expiryInfo.title = new Date(expires_at * 1000).toLocaleString();
    } else {
        expiryInfo.removeAttribute('title');
    }

    renderQR(currentShortUrl);

    document.getElementById('shorten-form').hidden = true;
    document.getElementById('result').hidden = false;
    document.getElementById('copy-btn').focus();
}

function relativeExpiry(expiresAt) {
    if (!expiresAt) return 'This link never expires';
    const secs = expiresAt - Math.floor(Date.now() / 1000);
    if (secs <= 0) return 'Expired';
    const days = Math.round(secs / 86400);
    if (days >= 365) {
        const years = Math.round(days / 365);
        return `Expires in ${years} year${years === 1 ? '' : 's'}`;
    }
    if (days >= 30) {
        const months = Math.round(days / 30);
        return `Expires in ${months} month${months === 1 ? '' : 's'}`;
    }
    if (days >= 1) return `Expires in ${days} day${days === 1 ? '' : 's'}`;
    const hours = Math.max(1, Math.round(secs / 3600));
    return `Expires in ${hours} hour${hours === 1 ? '' : 's'}`;
}

function renderQR(text) {
    const canvas = document.getElementById('qr-canvas');
    const ctx = canvas.getContext('2d');
    const size = canvas.width;

    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, size, size);

    try {
        const qr = qrcode(0, 'M');
        qr.addData(text);
        qr.make();
        const count = qr.getModuleCount();
        const cell = Math.floor(size / (count + 2));
        const offset = Math.floor((size - cell * count) / 2);
        ctx.fillStyle = '#000000';
        for (let r = 0; r < count; r++) {
            for (let c = 0; c < count; c++) {
                if (qr.isDark(r, c)) {
                    ctx.fillRect(offset + c * cell, offset + r * cell, cell, cell);
                }
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
    const copyBtn = document.getElementById('copy-btn');
    try {
        await navigator.clipboard.writeText(currentShortUrl);
        const original = copyBtn.textContent;
        copyBtn.textContent = 'Copied!';
        setTimeout(() => { copyBtn.textContent = original; }, 2000);
    } catch (err) {
        selectShortLink();
    }
}

function selectShortLink() {
    const link = document.getElementById('short-link');
    const range = document.createRange();
    range.selectNodeContents(link);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
}

function onOpen() {
    window.open(currentShortUrl, '_blank', 'noopener');
}

async function onShare() {
    try {
        await navigator.share({ url: currentShortUrl, title: 'Short link' });
    } catch (err) {
        if (err && err.name !== 'AbortError') {
            console.error('Share failed:', err);
        }
    }
}

function resetForm() {
    document.getElementById('url').value = '';
    document.getElementById('custom_code').value = '';
    document.getElementById('error').textContent = '';

    document.getElementById('result').hidden = true;
    document.getElementById('shorten-form').hidden = false;
    document.getElementById('url').focus();
}

function showError(msg) {
    document.getElementById('error').textContent = msg;
}

function setLoading(isLoading) {
    const btn = document.getElementById('shorten-btn');
    btn.disabled = isLoading;
    btn.textContent = isLoading ? 'Shortening...' : 'Shorten URL';
}
