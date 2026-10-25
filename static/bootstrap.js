import init, { start } from '/assets/contrix_frontend.js';

try {
    await init();
    start();
} catch (error) {
    console.error('Contrix Office failed to start:', error);
    const root = document.getElementById('main');
    root.textContent = 'Unable to load Contrix Office. Please reload the page.';
    root.setAttribute('role', 'alert');
}
