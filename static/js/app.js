document.addEventListener('DOMContentLoaded', () => {
    const shortenBtn = document.getElementById('shorten-btn');
    if (shortenBtn) {
        shortenBtn.addEventListener('click', shorten);
    }
    const copyBtn = document.getElementById('copy-btn');
    if (copyBtn) {
        copyBtn.addEventListener('click', copyToClipboard);
    }
});

async function shorten() {
    const url = document.getElementById('url').value.trim();
    const custom_code = document.getElementById('custom_code').value.trim();
    const errorDiv = document.getElementById('error');
    const resultDiv = document.getElementById('result');
    const shortLink = document.getElementById('short-link');

    errorDiv.textContent = '';
    resultDiv.style.display = 'none';

    if (!url) {
        showError('URL is required');
        return;
    }

    setLoading(true);

    try {
        const response = await fetch('/api/v1/shorten', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ url, custom_code: custom_code || null })
        });

        const data = await response.json();

        if (response.ok) {
            const fullShortUrl = window.location.origin + '/' + data.code;
            shortLink.href = fullShortUrl;
            shortLink.textContent = fullShortUrl;
            resultDiv.style.display = 'block';
        } else {
            showError(data.error || 'Something went wrong');
        }
    } catch (err) {
        showError('Failed to connect to the server');
    } finally {
        setLoading(false);
    }
}

function showError(msg) {
    const errorDiv = document.getElementById('error');
    errorDiv.textContent = msg;
}

function setLoading(isLoading) {
    const btn = document.getElementById('shorten-btn');
    if (isLoading) {
        btn.disabled = true;
        btn.textContent = 'Shortening...';
    } else {
        btn.disabled = false;
        btn.textContent = 'Shorten URL';
    }
}

async function copyToClipboard() {
    const link = document.getElementById('short-link').textContent;
    try {
        await navigator.clipboard.writeText(link);
        const copyBtn = document.getElementById('copy-btn');
        const originalText = copyBtn.textContent;
        copyBtn.textContent = 'Copied!';
        setTimeout(() => {
            copyBtn.textContent = originalText;
        }, 2000);
    } catch (err) {
        console.error('Failed to copy: ', err);
    }
}
