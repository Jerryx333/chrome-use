function (input) {
    const root = input.closest('.monaco-editor');
    if (!root || !root.isConnected || !input.isConnected) {
        throw Error('Monaco editor target is unavailable');
    }
    if (typeof this.getEditors !== 'function') return null;
    const editors = this.getEditors();
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
    return {
        focus() { check(); input.focus(); return checkFocus(); },
        checkFocus,
        read() { check(); return model.getValue(); },
    };
}
