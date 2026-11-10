import init, { calculate_workbook_formulas } from './contrix_frontend.js';

self.onmessage = async ({ data: [id, bytes] }) => {
    try {
        await init();
        const results = calculate_workbook_formulas(bytes);
        self.postMessage([id, results], [results.buffer]);
    } catch {
        self.postMessage([id, null]);
    }
};
