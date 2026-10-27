function (input, amd = false) {
    const root = input.closest('.monaco-editor');
    if (!root || !root.isConnected || !input.isConnected) {
        throw Error('Monaco editor target is unavailable');
    }
    let api = this;
    if (amd) {
        // Read only completed AMD data records. Never call the loader, a
        // module factory or an accessor, and never scan page globals. The
        // bounded table walk also works when the loader binding is minified.
        const own = (object, key) => object && Object.getOwnPropertyDescriptor(object, key)?.value;
        const modules = own(api, '_modules2');
        const length = own(modules, 'length');
        if (!Array.isArray(modules) || !Number.isInteger(length) || length > 4096) return null;
        api = null;
        for (let i = 0; i < length; i++) {
            const record = own(modules, String(i));
            if (own(record, 'strId') !== 'vs/editor/standalone/browser/standaloneEditor') continue;
            if (own(record, '_isComplete') !== true || own(record, 'error') !== null) return null;
            const exports = own(record, 'exports');
            if (typeof own(exports, 'getEditors') !== 'function') return null;
            api = exports;
            break;
        }
        if (!api) return null;
    }
    if (typeof api.getEditors !== 'function') return null;
    const editors = api.getEditors();
    if (!Array.isArray(editors)) return null;
    const matches = editors.filter(editor =>
        typeof editor.getDomNode === 'function' && editor.getDomNode() === root);
    if (matches.length === 0) return null;
    if (matches.length !== 1) throw Error('Monaco editor identity is ambiguous');
    const editor = matches[0];
    const model = editor.getModel();
    if (!model || typeof model.getValue !== 'function') {
        throw Error('Monaco editor has no readable model');
    }
    const check = () => {
        if (!root.isConnected || !input.isConnected || !root.contains(input)
            || editor.getDomNode() !== root || editor.getModel() !== model
            || (model.isDisposed && model.isDisposed())) {
            throw Error('Monaco editor or model changed; inspect it before retrying');
        }
    };
    const checkFocus = () => {
        check();
        if (input.disabled || input.readOnly || input.ownerDocument.activeElement !== input) {
            throw Error('Monaco input is unavailable or lost focus; input refused');
        }
        return true;
    };
    const checkSelection = () => {
        checkFocus();
        // A focused DOM input does not prove that the browser delivered the
        // select-all chord. Refuse before inserting into a partial selection.
        if (typeof editor.getSelection !== 'function' || typeof model.getFullModelRange !== 'function') {
            throw Error('Monaco cannot verify native selection; input refused');
        }
        const selection = editor.getSelection();
        const range = model.getFullModelRange();
        const fields = ['startLineNumber', 'startColumn', 'endLineNumber', 'endColumn'];
        if (!selection || !range || fields.some(key =>
            !Number.isInteger(range[key]) || range[key] < 1 || selection[key] !== range[key])) {
            throw Error('Monaco select-all did not select the full model; input refused');
        }
        return true;
    };
    return {
        focus() { check(); input.focus(); return checkFocus(); },
        checkFocus,
        checkSelection,
        read() { check(); return model.getValue(); },
    };
}
