# Third-party notices

## Orca

Parts of the Inspect & Ask feature are adapted from Orca
(https://github.com/stablyai/orca), commit
f31aada5b4f02e0a61d250716542518b9ce3cb83:

- `inspect-extract.js`: selector building, secret-pattern filter, computed-style
  subset and text budgets (from `src/main/browser/grab-guest-element-context-script.ts`,
  `src/main/browser/grab-guest-content-script.ts`, `src/shared/browser-grab-types.ts`).
- `inspect-probe.js`: React fiber walking and source-path cleaning (from
  `src/main/browser/grab-guest-react-script.ts`).
- `inspect-format.js`: fence and inline-code handling and the annotation layout
  (from `src/renderer/src/components/browser-pane/annotate/browser-annotation-output.ts`).

MIT License

Copyright (c) 2026 Lovecast Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
