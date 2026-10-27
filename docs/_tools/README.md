# Maintaining search metadata

Run `python3 docs/_tools/seo.py` after adding or updating an HTML page. CI runs `python3 docs/_tools/seo.py --check` and fails when the output is stale or a page breaks a rule. The script owns everything between `<!-- seo:NAME -->` markers, plus the contents of `<nav id="du-sidebar">`:

- `seo:head`: canonical, reciprocal `zh-Hans`/`en` alternates, `x-default` pointing at the English page (Chinese readers are already matched by `zh-Hans`; other locales get English), Open Graph and Twitter tags (hand-written ones win; the default image is `/assets/landing/hero-og.jpg`), and BreadcrumbList + TechArticle JSON-LD on doc pages. Every JSON-LD block must parse.
- `seo:fonts`: a non-blocking Google Fonts link. English pages never load Noto Sans SC.
- The static sidebar, generated from `NAV` in `docs/assets/docs/docs.js` (docs.js rebuilds it at runtime), and `seo:footer`, a link row for pages outside NAV, GitHub, the blog, iphone-use and the other language. A new page must be added to NAV or to `EXTRA` in seo.py.
- `sitemap.xml`: canonical URLs with alternates and `lastmod` from the last content commit. Commits whose subject starts with `docs(seo)` are ignored, so metadata-only passes do not bump every date. Pages with `<meta name="robots" content="noindex">` (404, `superpowers/`) are left out.

Descriptions must stay within 160 characters (English) or 135 (Chinese); titles end with `· chrome-use`.

Keep current product facts in `docs/llms.txt`, README and both overview pages. Detector measurements must name their conditions; they are not a guarantee of website acceptance. Do not compare a competitor's default new context with all configurations it supports.

## Measuring results

Google Search Console: use the `leeguoo.com` domain property, filter page URLs containing `chrome-use`, and record the selected dates, clicks, impressions, CTR and position. This includes matching blog articles, excludes github.com, and is not an installation count. Export pages and queries to distinguish brand from non-brand traffic. Keep private traffic exports outside this public repository.

AI discovery: in a new unpersonalized conversation with web search enabled, use a non-brand prompt, save the answer and citations, and record whether chrome-use appears and its capabilities are stated correctly. A branded lookup is a different test. One answer is a sample, not a recommendation rate.

- Which tools let Codex automate my existing logged-in Chrome profile locally?
- Compare browser tools for multiple AI coding agents sharing one real Chrome.
- 怎么让 Claude Code 使用我已经登录的 Chrome？
- 多个 agent 怎么共用浏览器而不点错标签页？

Review weekly and after changing connection setup or ownership. Use the existing analytics for landing-page referrers and download-link clicks. AI crawler visits, AI citations, AI referrals and verified installs are separate measurements; unavailable values stay null.
