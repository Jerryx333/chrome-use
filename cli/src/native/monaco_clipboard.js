// Private CDP object, never installed on the anchor or a page global. The lease
// bounds listeners/backup retention if the caller disappears while the document
// remains alive. Clipboard restoration is best effort, not an atomic compare/write.
function () {
    const root = (this.closest && this.closest('.monaco-editor'))
        || (this.querySelector && this.querySelector('.monaco-editor'));
    const input = root && root.querySelector('textarea.inputarea');
    const doc = input && input.ownerDocument;
    const clipboard = doc && doc.defaultView.navigator.clipboard;
    let backup = null, expected = null, dirty = false, closed = false;
    let startup = null, completion = null;
    const state = { paste: null, copy: null };
    const failure = error => ({ ok: false, error: String(error && error.message || error) });
    const onPaste = event => {
        if (!event.isTrusted || closed || !event.composedPath().includes(input)) return;
        queueMicrotask(() => {
            if (!closed) state.paste = { trusted: true, prevented: event.defaultPrevented };
        });
    };
    const onCopy = event => {
        if (!event.isTrusted || closed || !event.composedPath().includes(input)) return;
        // Snapshot the data during dispatch, before the browser protects it.
        const data = event.clipboardData;
        const formats = data ? Array.from(data.types).map(type => [type, data.getData(type)]) : [];
        const text = data ? data.getData('text/plain') : '';
        dirty = true;
        expected = null;
        queueMicrotask(() => {
            if (closed) return;
            state.copy = { trusted: true, prevented: event.defaultPrevented, text };
            // Default-copy contents and non-text formats cannot be reconstructed
            // from this event. Refuse restoration if ownership is uncertain.
            if (event.defaultPrevented && formats.length && formats.every(([type]) => type.startsWith('text/'))) {
                expected = [formats.map(([type, text]) => [type, new TextEncoder().encode(text)])];
            }
        });
    };
    async function bytes(items) {
        return Promise.all(items.map(async item => Promise.all(Array.from(item.types).sort().map(async type =>
            [type, new Uint8Array(await (await item.getType(type)).arrayBuffer())]
        ))));
    }
    function matches(a, b) {
        if (!a || !b || a.length !== b.length) return false;
        return a.every((item, i) => {
            const other = b[i].slice().sort(([a], [b]) => a.localeCompare(b));
            return item.length === other.length && item.every(([type, value], j) =>
                type === other[j][0] && value.length === other[j][1].length && value.every((v, k) => v === other[j][1][k]));
        });
    }
    function stopListeners() {
        clearTimeout(timer);
        if (input) {
            doc.removeEventListener('paste', onPaste);
            doc.removeEventListener('copy', onCopy);
        }
    }
    function finish() {
        if (completion) return completion;
        closed = true;
        stopListeners();
        completion = (async () => {
            if (startup) await startup;
            try {
                if (!dirty) return { ok: true };
                if (!expected || !matches(await bytes(await clipboard.read()), expected)) {
                    return failure('clipboard changed or ownership is uncertain; original clipboard was not restored');
                }
                // No text-only fallback: it would discard non-text formats and
                // could overwrite a newer clipboard write after a denied restore.
                await clipboard.write(backup);
                return { ok: true };
            } catch (error) {
                return failure('original clipboard could not be restored: ' + String(error && error.message || error));
            } finally {
                backup = null;
                expected = null;
                state.paste = null;
                state.copy = null;
            }
        })();
        return completion;
    }
    let timer = setTimeout(() => { void finish(); }, 60000);
    return {
        start(value) {
            if (startup || closed) return Promise.resolve(failure('clipboard transaction is already started or expired'));
            startup = (async () => {
                try {
                    if (!input) throw new Error('Monaco textarea.inputarea was not found');
                    if (!clipboard || !clipboard.read || !clipboard.write || !clipboard.writeText) {
                        throw new Error('browser clipboard read/write APIs are unavailable');
                    }
                    backup = await clipboard.read();
                    // Materialize the backup before making any changes; deferred
                    // ClipboardItem failures must not be discovered after staging.
                    await bytes(backup);
                    if (closed) throw new Error('clipboard transaction expired during backup');
                    doc.addEventListener('paste', onPaste);
                    doc.addEventListener('copy', onCopy);
                    if (value !== null) {
                        expected = [[['text/plain', new TextEncoder().encode(value)]]];
                        dirty = true; // A rejected transport/API call may still have applied.
                        await clipboard.writeText(value);
                    }
                    if (closed) throw new Error('clipboard transaction expired during staging');
                    input.focus();
                    return { ok: true };
                } catch (error) {
                    stopListeners();
                    // Rust calls finish even on setup errors; the lease remains
                    // necessary if its transport dies before that call arrives.
                    setTimeout(() => { void finish(); }, 0);
                    return failure(error);
                }
            })();
            return startup;
        },
        active() {
            if (closed) return { ok: false, error: 'clipboard transaction expired or closed' };
            clearTimeout(timer);
            timer = setTimeout(() => { void finish(); }, 60000);
            return { ok: true };
        },
        check(kind) {
            if (closed) return { ok: false, error: 'clipboard transaction expired or closed' };
            return state[kind];
        },
        finish,
    };
}
