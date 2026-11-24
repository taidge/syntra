import init, { start } from '/assets/syntra_frontend.js';

try {
    await init();
    start();
    // Glory installs its host reset during launch. Keep that reset below Office's
    // stylesheet so semantic headings retain their original sizes and spacing.
    const baseStyle = document.querySelector('style[data-gly-base="v1"]');
    if (baseStyle) {
        baseStyle.textContent = `@layer glory-base { ${baseStyle.textContent} }`;
    }
} catch (error) {
    console.error('Syntra Office failed to start:', error);
    const root = document.getElementById('main');
    root.textContent = 'Unable to load Syntra Office. Please reload the page.';
    root.setAttribute('role', 'alert');
}
