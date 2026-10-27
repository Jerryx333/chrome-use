#!/usr/bin/env python3
"""Materialize crawler-visible metadata, static navigation and the sitemap.

Run from any directory:  python3 docs/_tools/seo.py          (write)
                         python3 docs/_tools/seo.py --check  (CI: fail if stale)

Everything this script writes lives between `<!-- seo:NAME -->` markers (plus
the static sidebar inside `<nav id="du-sidebar">`), so a re-run is byte-stable.
The page list and labels come from NAV in docs/assets/docs/docs.js, the same
source the runtime sidebar uses.
"""
from pathlib import Path
import datetime, html, json, re, subprocess, sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
DOCS = ROOT / 'docs'
ORIGIN = 'https://chrome-use.leeguoo.com'
OG_IMAGE = ORIGIN + '/assets/landing/hero-og.jpg'
GH = 'https://github.com/leeguooooo/chrome-use'
PERSON = 'https://leeguoo.com/about#person'
NS = 'http://www.sitemaps.org/schemas/sitemap/0.9'
ALT = 'http://www.w3.org/1999/xhtml'
ET.register_namespace('', NS)
ET.register_namespace('xhtml', ALT)
check = '--check' in sys.argv
changes, problems = [], []

# ------------------------------------------------------------ NAV from docs.js
js = (DOCS / 'assets/docs/docs.js').read_text(encoding='utf-8')
nav_src = js[js.index('var NAV = ['):js.index('var TABS')]
NAV = []
for m in re.finditer(r'\{\s*id:\s*"(\w+)",\s*label:\s*\{\s*zh:\s*"([^"]+)",\s*en:\s*"([^"]+)"\s*\}|'
                     r'\{\s*slug:\s*"([\w-]+)",\s*ico:\s*"([^"]+)",\s*zh:\s*"([^"]+)",\s*en:\s*"([^"]+)"', nav_src):
    if m[1]:
        NAV.append({'id': m[1], 'zh': m[2], 'en': m[3], 'items': []})
    else:
        NAV[-1]['items'].append({'slug': m[4], 'ico': m[5], 'zh': m[6], 'en': m[7]})
GROUP_OF = {it['slug']: g for g in NAV for it in g['items']}
ITEM = {it['slug']: it for g in NAV for it in g['items']}
# Indexable pages that are not in the sidebar: linked from the static footer.
EXTRA = {
    'zh': [('/logged-in-browser.html', '让 agent 用已登录 Chrome'), ('/debugger-banner-silence.html', '静音调试提示条'),
           ('/privacy.html', '隐私政策'), ('/mobile-privacy.html', '手机 App 隐私政策')],
    'en': [('/en/logged-in-browser.html', 'Use your logged-in Chrome from an agent'),
           ('/en/privacy.html', 'Privacy policy'), ('/mobile-privacy.html', 'Mobile app privacy policy')],
}
OUT = {
    'zh': [(GH, 'GitHub'), (GH + '/releases', '下载 / Releases'), ('https://iphone-use.leeguoo.com/', 'iphone-use（让 agent 操作 iPhone）'),
           ('https://blog.leeguoo.com', '博客')],
    'en': [(GH, 'GitHub'), (GH + '/releases', 'Releases'), ('https://iphone-use.leeguoo.com/en/', 'iphone-use (drive an iPhone)'),
           ('https://blog.leeguoo.com/en', 'Blog')],
}


def route(slug, en):
    return ('/en/' if en else '/') + ('' if slug in ('index', 'overview') else slug + '.html')


def esc(s):
    return html.escape(s, quote=True)


def text_of(fragment):
    return html.unescape(re.sub(r'<[^>]+>', '', fragment)).strip()


def is_en(src):
    return bool(re.search(r'<html[^>]+lang=["\']en', src))


def noindex(src):
    return bool(re.search(r'<meta[^>]+name=["\']robots["\'][^>]+noindex', src))


def sidebar(slug, en):
    lng = 'en' if en else 'zh'
    out = []
    for g in NAV:
        out.append(f'<div class="du-nav-group"><div class="du-nav-label">{esc(g[lng])}</div>')
        for it in g['items']:
            cur = ' active" aria-current="page' if it['slug'] == slug else ''
            out.append(f'<a class="du-nav-item{cur}" href="{route(it["slug"], en)}"><span class="du-nav-ico">{it["ico"]}</span>'
                       f'<span class="du-nav-text">{esc(it[lng])}</span></a>')
        out.append('</div>')
    return ''.join(out)


def footer(rel, en, mirror_href):
    lng = 'en' if en else 'zh'
    links = [f'<a href="{h}">{esc(t)}</a>' for h, t in EXTRA[lng]]
    links += [f'<a href="{h}">{esc(t)}</a>' for h, t in OUT[lng]]
    if mirror_href:
        links.append(f'<a href="{mirror_href}" hreflang="{"zh-Hans" if en else "en"}">{"中文" if en else "English"}</a>')
    label = 'More' if en else '更多'
    return (f'<!-- seo:footer --><nav class="du-static-footer" aria-label="{label}">'
            f'<a href="{route("index", en)}">chrome-use</a> · ' + ' · '.join(links) + '</nav><!-- /seo:footer -->')


FONTS_PLACEHOLDER = '<link href="https://fonts.googleapis.com/css2?family=" rel="stylesheet" />'


def strip_managed(s):
    # The fonts block is rewritten in place, so reduce it (or a hand-written
    # Google Fonts link) to a fixed placeholder that generation replaces again.
    s = re.sub(r'<!-- seo:fonts -->.*?<!-- /seo:fonts -->', FONTS_PLACEHOLDER, s, flags=re.S)
    s = re.sub(r'<link href="https://fonts\.googleapis\.com/css2\?[^"]*" rel="stylesheet" />', FONTS_PLACEHOLDER, s)
    s = re.sub(r'\s*<!-- seo:(\w+) -->.*?<!-- /seo:\1 -->', '', s, flags=re.S)
    s = re.sub(r'\s*<link\b[^>]*\brel=["\'](?:canonical|alternate)["\'][^>]*>', '', s)
    s = re.sub(r'<nav aria-label="(?:Documentation|文档)">.*?</nav>', '', s, flags=re.S)
    s = re.sub(r'<nav id="du-sidebar"[^>]*>.*?</nav>', '<nav id="du-sidebar"></nav>', s, flags=re.S)
    s = s.replace('<div id="du-topbar" class="du-topbar"></div>', '<div id="du-topbar"></div>')
    return s


def git_date(p):
    # Last content commit, ignoring metadata-only "docs(seo)" passes so a site-wide
    # regeneration does not bump every page's lastmod.
    return subprocess.check_output(['git', 'log', '-1', '--format=%cs', '--invert-grep', '--grep=^docs(seo)', '--', str(p)],
                                   cwd=ROOT, text=True).strip()


def content_date(p, source):
    rel = p.relative_to(ROOT).as_posix()
    try:
        head = subprocess.check_output(['git', 'show', f'HEAD:{rel}'], cwd=ROOT, text=True, stderr=subprocess.DEVNULL)
    except subprocess.CalledProcessError:
        return datetime.date.today().isoformat()
    if strip_managed(head) != strip_managed(source):
        return datetime.date.today().isoformat()
    return git_date(p) or datetime.date.today().isoformat()


urls = ET.Element(f'{{{NS}}}urlset')
seen = set()
pages = sorted([*DOCS.glob('*.html'), *DOCS.glob('en/*.html')])
for slug_ in ITEM:
    for en in (False, True):
        if not (DOCS / route(slug_, en).lstrip('/')).exists() and slug_ != 'overview':
            problems.append(f'NAV slug {slug_} has no {"en" if en else "zh"} page')

for p in pages:
    rel = p.relative_to(DOCS).as_posix()
    slug = p.stem
    en = rel.startswith('en/')
    source = p.read_text(encoding='utf-8')
    actual_en = is_en(source)
    canonical = ORIGIN + route(slug, en)
    mirror = DOCS / (slug + '.html') if en else DOCS / 'en' / (slug + '.html')
    zh_page = DOCS / (slug + '.html')
    has_zh = zh_page.exists() and not is_en(zh_page.read_text(encoding='utf-8'))
    # Some legacy root pages are English aliases of an /en/ page.
    if actual_en and not en and mirror.exists():
        canonical = ORIGIN + route(slug, True)
    hidden = noindex(source)
    paired = mirror.exists() and has_zh and not hidden
    # x-default -> English: zh readers are already routed by zh-Hans; unmatched
    # locales (ja, de, ...) are better served by the English docs.
    alts = [('zh-Hans', False), ('en', True), ('x-default', True)] if paired else []
    page_slug = (re.search(r'data-page="([\w-]+)"', source) or [None, slug])[1]
    lng = 'en' if actual_en else 'zh'
    if not hidden and page_slug not in ITEM and slug not in ('index', 'privacy', 'mobile-privacy', 'logged-in-browser', 'debugger-banner-silence'):
        problems.append(f'{rel}: not in docs.js NAV or the static footer')

    clean = strip_managed(source)
    title = text_of(re.search(r'<title>(.*?)</title>', clean, re.S)[1])
    dm = re.search(r'<meta\s+name="description"\s+content="([^"]*)"', clean)
    desc = html.unescape(dm[1]) if dm else ''
    if not hidden:
        if not desc:
            problems.append(f'{rel}: missing meta description')
        elif len(desc) > (160 if actual_en else 135):
            problems.append(f'{rel}: description is {len(desc)} chars')
        if not title.endswith('chrome-use') and slug != 'index' and 'chrome-use' not in title:
            problems.append(f'{rel}: title lacks the chrome-use suffix')

    # ---------------------------------------------------------------- <head>
    is404 = rel == '404.html'  # served at any missing path: no canonical
    head = [] if is404 else [f'<link rel="canonical" href="{canonical}" />']
    for hl, to_en in alts:
        head.append(f'<link rel="alternate" hreflang="{hl}" href="{ORIGIN + route(slug, to_en)}" />')
    def has(prop):
        return re.search(r'<meta[^>]+(?:property|name)="' + re.escape(prop) + '"', clean)
    og = [('og:type', 'website' if slug == 'index' else 'article'),
          ('og:site_name', 'chrome-use'), ('og:title', title), ('og:description', desc), ('og:url', canonical),
          ('og:image', OG_IMAGE), ('og:image:width', '1200'), ('og:image:height', '800'),
          ('og:image:alt', 'chrome-use: AI agents driving your logged-in Chrome' if actual_en else 'chrome-use：让 AI agent 用你已登录的 Chrome'),
          ('og:locale', 'en_US' if actual_en else 'zh_CN')]
    if paired:
        og.append(('og:locale:alternate', 'zh_CN' if actual_en else 'en_US'))
    img_custom = has('og:image')
    for k, v in ([] if is404 else og):
        if k.startswith('og:image:') and img_custom:
            continue
        if not has(k) and v:
            head.append(f'<meta property="{k}" content="{esc(v)}" />')
    tw = [('twitter:card', 'summary_large_image'), ('twitter:title', title), ('twitter:description', desc),
          ('twitter:image', (re.search(r'<meta property="og:image" content="([^"]+)"', clean) or [None, OG_IMAGE])[1])]
    for k, v in ([] if is404 else tw):
        if not has(k) and v:
            head.append(f'<meta name="{k}" content="{esc(v)}" />')
    if not hidden and slug != 'index':
        h1 = re.search(r'<h1\b[^>]*>(.*?)</h1>', clean, re.S)
        crumbs = [('chrome-use', ORIGIN + route('index', actual_en))]
        g = GROUP_OF.get(page_slug)
        if g:
            first = g['items'][0]['slug']
            crumbs.append((g[lng], ORIGIN + route(first, actual_en)))
        crumbs.append((text_of(h1[1]) if h1 else title, canonical))
        if len(crumbs) == 3 and crumbs[1][1] in (canonical, crumbs[0][1]):
            crumbs.pop(1)
        ld = [{'@context': 'https://schema.org', '@type': 'BreadcrumbList', 'itemListElement': [
            {'@type': 'ListItem', 'position': i + 1, 'name': n, 'item': u} for i, (n, u) in enumerate(crumbs)]}]
        if 'docs.js' in clean:
            ld.append({'@context': 'https://schema.org', '@type': 'TechArticle',
                       'headline': (text_of(h1[1]) if h1 else title)[:110], 'description': desc,
                       'inLanguage': 'en' if actual_en else 'zh-Hans', 'url': canonical, 'mainEntityOfPage': canonical,
                       'image': OG_IMAGE, 'dateModified': content_date(p, source),
                       'author': {'@id': PERSON, '@type': 'Person', 'name': 'Guo Li (leeguoo)', 'url': 'https://leeguoo.com/about'},
                       'publisher': {'@id': PERSON, '@type': 'Person', 'name': 'Guo Li (leeguoo)', 'url': 'https://leeguoo.com/about'},
                       'isPartOf': {'@type': 'WebSite', 'name': 'chrome-use', 'url': ORIGIN + '/'},
                       'about': {'@type': 'SoftwareApplication', 'name': 'chrome-use', 'url': ORIGIN + '/'}})
        for obj in ld:
            head.append('<script type="application/ld+json">' + json.dumps(obj, ensure_ascii=False, separators=(',', ':')) + '</script>')
    block = '<!-- seo:head -->\n  ' + '\n  '.join(head) + '\n  <!-- /seo:head -->'
    result = clean.replace('</head>', '  ' + block + '\n</head>', 1) if head else clean

    # ---------------------------------------------------------------- fonts
    # English pages never load the CJK body font; both drop unused weights and
    # load the stylesheet without blocking first render (font-display: swap).
    fam = 'family=ZCOOL+KuaiLe&family=Permanent+Marker' + ('' if actual_en else '&family=Noto+Sans+SC:wght@400;700') + \
          '&family=JetBrains+Mono:wght@400;700&display=swap'
    href = 'https://fonts.googleapis.com/css2?' + fam
    fonts = (f'<!-- seo:fonts --><link rel="preload" as="style" href="{href}" onload="this.onload=null;this.rel=\'stylesheet\'" />'
             f'<noscript><link rel="stylesheet" href="{href}" /></noscript><!-- /seo:fonts -->')
    result = re.sub(r'<link href="https://fonts\.googleapis\.com/css2\?[^"]*" rel="stylesheet" />', fonts, result, count=1)

    # ---------------------------------------------------------------- static navigation
    result = result.replace('<div id="du-topbar"></div>', '<div id="du-topbar" class="du-topbar"></div>')
    result = re.sub(r'<nav id="du-sidebar"[^>]*></nav>',
                    lambda m: f'<nav id="du-sidebar" class="du-sidebar" aria-label="{"Documentation" if actual_en else "文档"}">'
                              + sidebar(page_slug, actual_en) + '</nav>', result, count=1)
    mirror_href = route(slug, not en) if paired else None
    foot = footer(rel, actual_en, mirror_href)
    if '</main>' in result:
        result = result.replace('</main>', foot + '</main>', 1)
    else:
        result = result.replace('</body>', foot + '\n</body>', 1)

    for blob in re.findall(r'<script type="application/ld\+json">(.*?)</script>', result, re.S):
        try:
            json.loads(blob)
        except ValueError as e:
            problems.append(f'{rel}: invalid JSON-LD ({e})')

    if result != source:
        if check:
            norm = lambda s: re.sub(r'"dateModified":"[\d-]+"', '', s)
            if norm(result) != norm(source):
                changes.append(rel)
        else:
            changes.append(rel)
            p.write_text(result, encoding='utf-8')
    if hidden or canonical in seen:
        continue
    seen.add(canonical)
    item = ET.SubElement(urls, f'{{{NS}}}url')
    ET.SubElement(item, f'{{{NS}}}loc').text = canonical
    for hl, to_en in alts:
        ET.SubElement(item, f'{{{ALT}}}link', {'rel': 'alternate', 'hreflang': hl, 'href': ORIGIN + route(slug, to_en)})
    date = content_date(p, source)
    if date:
        ET.SubElement(item, f'{{{NS}}}lastmod').text = date

ET.indent(urls, space='  ')
sitemap = '<?xml version="1.0" encoding="UTF-8"?>\n' + ET.tostring(urls, encoding='unicode') + '\n'
if check:
    # lastmod may change after commits: validate the URL set rather than demand churn.
    existing = ET.parse(DOCS / 'sitemap.xml')
    if set(x.text for x in existing.findall('.//{*}loc')) != set(x.text for x in urls.findall('.//{*}loc')):
        changes.append('sitemap URL coverage')

    def alternates(tree):
        return {e.find('{*}loc').text: {(l.get('hreflang'), l.get('href')) for l in e.findall('{*}link')} for e in tree.findall('.//{*}url')}
    if alternates(existing) != alternates(urls):
        changes.append('sitemap language alternates')
else:
    (DOCS / 'sitemap.xml').write_text(sitemap, encoding='utf-8')
if problems:
    raise SystemExit('SEO problems:\n  ' + '\n  '.join(problems))
if check and changes:
    raise SystemExit('Stale SEO metadata (run python3 docs/_tools/seo.py): ' + ', '.join(changes))
print(f'SEO metadata verified for {len(pages)} pages' if check else f'Materialized SEO metadata for {len(pages)} pages ({len(changes)} changed)')
