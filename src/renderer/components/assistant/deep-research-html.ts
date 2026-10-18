/**
 * Standalone HTML export for deep research articles — the "default" page code.
 *
 * The model only fills DeepResearchData (JSON); this fixed template guarantees
 * the professional magazine design (based on deep-research-sample.html): sticky
 * MangaVox header, full-bleed hero, byline, article + trending sidebar grid,
 * star ratings, key takeaways, recommended grid, sources, footer, scroll-reveal
 * animation, print support — with automatic dark/light theme.
 */
import type { DeepResearchData } from './DeepResearchArticle'

function esc(s: unknown): string {
  return String(s ?? '').replace(/[&<>"']/g, (c) => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c] || c
  ))
}

export function buildDeepResearchHtml(data: DeepResearchData): string {
  const title = esc(data.heroTitle || 'Deep Research')
  const date = esc(data.publishedDate || new Date().toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' }))

  const gradients = [
    "linear-gradient(145deg,#0083b0,#00b4db)",
    "linear-gradient(145deg,#ff758c,#ff7eb3)",
    "linear-gradient(145deg,#11998e,#38ef7d)",
    "linear-gradient(145deg,#7f00ff,#e100ff)",
    "linear-gradient(145deg,#f7971e,#ffd200)",
    "linear-gradient(145deg,#232526,#b31217)",
    "linear-gradient(145deg,#2193b0,#6dd5ed)",
    "linear-gradient(145deg,#614385,#516395)",
    "linear-gradient(145deg,#56ccf2,#2f80ed)",
    "linear-gradient(145deg,#8e9eab,#eef2f3)",
    "linear-gradient(145deg,#0f2027,#2c5364)",
    "linear-gradient(145deg,#f12711,#f5af19)",
    "linear-gradient(145deg,#3a6073,#16222a)"
  ];

  const mappedManga = (data.sections || []).map((sec, i) => {
    // 1. parse metadata if present
    let creator = data.author || "MangaVox Editorial";
    let publisher = "MangaVox";
    let status = "Verified";
    let time = data.readTime || "5 min read";

    if (sec.meta) {
      const parts = sec.meta.split(/[·•|]/).map(p => p.trim());
      if (parts.length > 0 && parts[0]) creator = parts[0];
      if (parts.length > 1 && parts[1]) publisher = parts[1];
      if (parts.length > 2 && parts[2]) status = parts[2];
      if (parts.length > 3 && parts[3]) time = parts[3];
    }

    // 2. infer genre from title/paragraphs
    const textToScan = (sec.title + " " + (sec.paragraphs || []).join(" ")).toLowerCase();
    let genre = "";
    if (textToScan.includes("romance") || textToScan.includes("romantic") || textToScan.includes("love story")) {
      genre = "Romance";
    } else if (textToScan.includes("fantasy") || textToScan.includes("magic") || textToScan.includes("witch") || textToScan.includes("dungeon")) {
      genre = "Fantasy";
    } else if (textToScan.includes("action") || textToScan.includes("fight") || textToScan.includes("battle") || textToScan.includes("assassin") || textToScan.includes("sword") || textToScan.includes("combat")) {
      genre = "Action";
    } else if (textToScan.includes("horror") || textToScan.includes("scary") || textToScan.includes("ghost") || textToScan.includes("unsettling") || textToScan.includes("supernatural")) {
      genre = "Horror";
    } else if (textToScan.includes("sport") || textToScan.includes("skate") || textToScan.includes("badminton") || textToScan.includes("athlete") || textToScan.includes("coach")) {
      genre = "Sports";
    } else if (textToScan.includes("slice of life") || textToScan.includes("supermarket") || textToScan.includes("salaryman") || textToScan.includes("casual")) {
      genre = "Slice of Life";
    } else {
      genre = data.category || "Research";
    }

    // 3. extract strengths (lines starting with - / * / bullet / numbers)
    let strengths: string[] = [];
    for (const p of sec.paragraphs) {
      const lines = p.split('\n');
      for (const line of lines) {
        const trimmed = line.trim();
        if (trimmed.startsWith('-') || trimmed.startsWith('*') || /^\d+\./.test(trimmed)) {
          strengths.push(trimmed.replace(/^[\-*\d.\s]+/, ''));
        }
      }
    }
    if (strengths.length === 0) {
      const sentences = sec.paragraphs.join(' ').split(/[.!?]+/).map(s => s.trim()).filter(s => s.length > 15);
      if (sentences.length >= 3) {
        strengths = sentences.slice(1, 4);
      } else {
        strengths = [
          "Rich narrative depth and pacing",
          "Highly detailed art style and layout",
          "Unique character dynamics and thematic focus"
        ];
      }
    }

    // 4. overview
    const overview = sec.paragraphs.filter(p => !p.trim().startsWith('-') && !p.trim().startsWith('*') && !/^\d+\./.test(p.trim())).join('\n\n');

    // 5. slug
    const titleCleaned = sec.title.replace(/^\d+[\s.\-:]+/, '');
    const slug = titleCleaned.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/(^-|-$)/g, '') || `section-${i + 1}`;

    // 6. verdict (the last paragraph, or fallback)
    let verdict = "Highly recommended entry point for new and returning fans alike.";
    const paragraphsFiltered = sec.paragraphs.filter(p => !p.trim().startsWith('-') && !p.trim().startsWith('*') && !/^\d+\./.test(p.trim()));
    if (paragraphsFiltered.length > 0) {
      verdict = paragraphsFiltered[paragraphsFiltered.length - 1];
    }

    // 7. source & url
    const sourceObj = data.sources && data.sources[i] ? data.sources[i] : (data.sources && data.sources[0] ? data.sources[0] : null);
    const sourceTitle = sourceObj ? sourceObj.title : (sec.imageCredit || "Official Publisher Page");
    const sourceUrl = sourceObj ? sourceObj.url : (sec.image || "https://manga-vox.com");

    // 8. best for
    const best = (data.keyTakeaways && data.keyTakeaways[i]) || "Readers seeking depth and quality";

    return {
      rank: i + 1,
      title: titleCleaned,
      slug,
      creator,
      genre,
      tone: sec.rating ? `${sec.rating} / 5 Stars` : "Insightful and detailed",
      status,
      publisher,
      best,
      time,
      hook: sec.paragraphs[0] || "",
      overview,
      strengths,
      verdict,
      source: sourceTitle,
      url: sourceUrl,
      grad: gradients[i % gradients.length],
      image: sec.image || ""
    };
  });

  const sec1_title = data.sections[0] ? data.sections[0].title.replace(/^\d+[\s.\-:]+/, '') : 'Adventure';
  const sec1_category = data.sections[0] ? (data.sections[0].meta ? data.sections[0].meta.split(/[·•|]/)[0].trim() : data.category || 'Manga') : 'WORLD';
  const sec1_image = data.sections[0]?.image || '';

  const sec2_title = data.sections[1] ? data.sections[1].title.replace(/^\d+[\s.\-:]+/, '') : 'Romance';
  const sec2_category = data.sections[1] ? (data.sections[1].meta ? data.sections[1].meta.split(/[·•|]/)[0].trim() : data.category || 'Manga') : 'HEART';
  const sec2_image = data.sections[1]?.image || '';

  const sec3_title = data.sections[2] ? data.sections[2].title.replace(/^\d+[\s.\-:]+/, '') : 'Stories';
  const sec3_category = data.sections[2] ? (data.sections[2].meta ? data.sections[2].meta.split(/[·•|]/)[0].trim() : data.category || 'Manga') : 'INK';
  const sec3_image = data.sections[2]?.image || '';

  const heroArtStyle = data.heroImage
    ? `style="background: linear-gradient(135deg, rgba(17,21,34,0.6), rgba(17,21,34,0.9)), url('${esc(data.heroImage)}') center/cover;"`
    : '';

  const c1_style = sec1_image ? `style="background: linear-gradient(0deg, rgba(0,0,0,0.78), rgba(0,0,0,0.25)), url('${esc(sec1_image)}') center/cover;"` : '';
  const c2_style = sec2_image ? `style="background: linear-gradient(0deg, rgba(0,0,0,0.78), rgba(0,0,0,0.25)), url('${esc(sec2_image)}') center/cover;"` : '';
  const c3_style = sec3_image ? `style="background: linear-gradient(0deg, rgba(0,0,0,0.78), rgba(0,0,0,0.25)), url('${esc(sec3_image)}') center/cover;"` : '';

  const sourcesHtml = (data.sources || []).map((src, i) => `
    <div class="source-row" id="source-${i + 1}">
      <div class="source-no">${i + 1}</div>
      <div><strong>${esc(src.title)}</strong><span>${esc(src.domain || src.url)}</span></div>
      <a href="${esc(src.url)}" target="_blank" rel="noopener">Open ↗</a>
    </div>`).join('\n');

  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${title} | MangaVox</title>
<meta name="description" content="A dynamic MangaVox editorial guide featuring ${data.sections.length} recommendations, genre filters, connected navigation, detailed reviews, and expandable verified sources.">
<meta name="author" content="${esc(data.author || 'MangaVox Editorial Board')}">
<meta name="theme-color" content="#111827">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=DM+Sans:wght@400;500;600;700&family=Manrope:wght@500;600;700;800&family=Newsreader:opsz,wght@6..72,400;6..72,500;6..72,600&display=swap" rel="stylesheet">
<style>
:root{
  --page:#f3f5f9;--surface:#fff;--ink:#131722;--body:#404757;--muted:#747c8d;
  --line:#e2e6ed;--indigo:#5b4ff7;--indigo-2:#4438d7;--rose:#ef476f;
  --soft:#f0efff;--dark:#111522;--green:#138a6b;--radius:18px;
  --shadow:0 14px 40px rgba(25,31,49,.09);--max:1280px;
}
*{box-sizing:border-box;margin:0;padding:0}html{scroll-behavior:smooth;scroll-padding-top:96px}
body{background:var(--page);color:var(--ink);font-family:"DM Sans",sans-serif;line-height:1.65;-webkit-font-smoothing:antialiased}
a{color:inherit;text-decoration:none}button,input{font:inherit}button{cursor:pointer}
.progress{position:fixed;inset:0 0 auto;height:3px;z-index:200;background:transparent}.progress span{display:block;height:100%;width:0;background:linear-gradient(90deg,var(--indigo),var(--rose))}
.site-header{position:sticky;top:0;z-index:100;background:rgba(255,255,255,.9);backdrop-filter:blur(18px);border-bottom:1px solid rgba(226,230,237,.9)}
.nav{max-width:var(--max);height:76px;margin:auto;padding:0 26px;display:flex;align-items:center;gap:28px}.logo{font:800 1.45rem "Manrope";letter-spacing:-.04em}.logo b{color:var(--indigo)}
.navlinks{margin-left:auto;display:flex;gap:26px;font-weight:700;font-size:.9rem;color:#565d6d}.navlinks a:hover{color:var(--indigo)}
.nav-search{width:210px;border:1px solid var(--line);background:#f7f8fb;border-radius:999px;padding:10px 15px;outline:none}.nav-search:focus{border-color:var(--indigo);box-shadow:0 0 0 4px rgba(91,79,247,.1)}
.mobile-menu{display:none;margin-left:auto;background:var(--dark);color:#fff;border:0;border-radius:10px;padding:9px 13px}
.hero{max-width:var(--max);margin:28px auto 32px;padding:0 26px}.crumbs{font-size:.84rem;color:var(--muted);margin-bottom:24px}.crumbs a:hover{color:var(--indigo)}
.hero-panel{min-height:520px;border-radius:28px;overflow:hidden;display:grid;grid-template-columns:1.08fr .92fr;background:var(--dark);box-shadow:var(--shadow)}
.hero-copy{padding:clamp(42px,6vw,78px);color:#fff;display:flex;flex-direction:column;justify-content:center}.eyebrow{display:flex;align-items:center;gap:10px;font-weight:800;font-size:.78rem;letter-spacing:.12em;text-transform:uppercase;color:#bcb6ff}.eyebrow:before{content:"";width:34px;height:2px;background:var(--rose)}
.hero h1{font:800 clamp(2.7rem,5.2vw,5.25rem)/.98 "Manrope";letter-spacing:-.06em;margin:20px 0 24px}.hero-copy>p{color:#bec4d3;font:400 clamp(1.05rem,1.4vw,1.25rem)/1.65 "Newsreader";max-width:670px}
.hero-meta{display:flex;align-items:center;gap:12px;margin-top:30px;color:#d4d8e2;font-size:.9rem}.avatar{width:44px;height:44px;border-radius:50%;background:linear-gradient(135deg,#ffe08a,#ef476f 55%,#5b4ff7);border:2px solid rgba(255,255,255,.3)}
.hero-art{position:relative;min-height:520px;background:radial-gradient(circle at 35% 20%,rgba(240,71,111,.75),transparent 30%),radial-gradient(circle at 85% 80%,rgba(91,79,247,.85),transparent 38%),linear-gradient(145deg,#1f2638,#0b0e17);overflow:hidden}
.cover-stack{position:absolute;inset:0;display:grid;place-items:center;transform:rotate(5deg)}.cover{position:absolute;width:min(260px,58%);aspect-ratio:2/3;border-radius:16px;border:1px solid rgba(255,255,255,.18);box-shadow:0 26px 70px rgba(0,0,0,.45);display:flex;flex-direction:column;justify-content:flex-end;padding:24px;color:white;overflow:hidden}.cover:after{content:"";position:absolute;inset:0;background:linear-gradient(0deg,rgba(0,0,0,.78),transparent 60%)}.cover>*{position:relative;z-index:1}.cover small{font-weight:800;letter-spacing:.12em}.cover strong{font:800 2rem/1 "Manrope";margin-top:7px}.c1{transform:translate(-38%,-5%) rotate(-13deg);background:linear-gradient(150deg,#11998e,#38ef7d)}.c2{transform:translate(30%,6%) rotate(9deg);background:linear-gradient(150deg,#ff512f,#dd2476)}.c3{transform:translate(0,-3%) rotate(-1deg);background:linear-gradient(150deg,#654ea3,#eaafc8)}
.trustbar{max-width:var(--max);margin:0 auto 44px;padding:0 26px;display:grid;grid-template-columns:repeat(4,1fr);gap:14px}.trust{background:#fff;border:1px solid var(--line);padding:18px 20px;border-radius:14px}.trust strong{display:block;font:800 1.25rem "Manrope"}.trust span{color:var(--muted);font-size:.84rem}
.explorer{max-width:var(--max);margin:0 auto 52px;padding:0 26px}.section-kicker{color:var(--indigo);font-weight:800;text-transform:uppercase;letter-spacing:.12em;font-size:.76rem}.section-title{font:800 clamp(2rem,3.5vw,3.25rem)/1.1 "Manrope";letter-spacing:-.045em;margin:10px 0 12px}.section-sub{color:var(--muted);max-width:700px}
.toolbar{display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin:26px 0 20px}.searchbox{flex:1;min-width:250px;position:relative}.searchbox input{width:100%;background:#fff;border:1px solid var(--line);border-radius:13px;padding:14px 16px 14px 44px;outline:none}.searchbox:before{content:"⌕";position:absolute;left:16px;top:8px;font-size:1.4rem;color:var(--muted)}.searchbox input:focus{border-color:var(--indigo);box-shadow:0 0 0 4px rgba(91,79,247,.09)}
.filters{display:flex;gap:8px;flex-wrap:wrap}.filter{border:1px solid var(--line);background:#fff;color:#596071;border-radius:999px;padding:10px 15px;font-weight:700;font-size:.84rem}.filter.active,.filter:hover{background:var(--dark);color:#fff;border-color:var(--dark)}.result-count{font-size:.86rem;color:var(--muted);margin-bottom:18px}
.card-grid{display:grid;grid-template-columns:repeat(4,1fr);gap:18px}.m-card{background:#fff;border:1px solid var(--line);border-radius:17px;overflow:hidden;box-shadow:0 5px 18px rgba(25,31,49,.0455);transition:.28s ease;display:flex;flex-direction:column}.m-card:hover{transform:translateY(-6px);box-shadow:var(--shadow);border-color:#cbc6ff}.mini-cover{height:178px;padding:20px;display:flex;flex-direction:column;justify-content:space-between;color:#fff;position:relative;overflow:hidden}.mini-cover:before{content:"";position:absolute;width:130px;height:130px;border:24px solid rgba(255,255,255,.12);border-radius:50%;right:-42px;top:-45px}.mini-cover .rank{font:800 2.5rem "Manrope";opacity:.72}.mini-cover .genre{align-self:flex-start;background:rgba(0,0,0,.18);backdrop-filter:blur(6px);padding:6px 9px;border-radius:999px;font-size:.72rem;font-weight:800;letter-spacing:.05em;text-transform:uppercase}.card-body{padding:18px;display:flex;flex-direction:column;flex:1}.m-card h3{font:800 1.08rem/1.25 "Manrope";letter-spacing:-.02em}.creator{color:var(--muted);font-size:.8rem;margin:6px 0 13px}.hook{color:#555d6c;font:400 .95rem/1.55 "Newsreader";flex:1}.card-link{margin-top:16px;color:var(--indigo);font-weight:800;font-size:.84rem;display:flex;justify-content:space-between}.empty{display:none;background:#fff;border:1px dashed var(--line);padding:36px;border-radius:16px;text-align:center;color:var(--muted)}
.main-layout{max-width:var(--max);margin:0 auto;padding:0 26px;display:grid;grid-template-columns:275px minmax(0,1fr);gap:54px;align-items:start}.sidebar{position:sticky;top:100px}.side-box{background:#fff;border:1px solid var(--line);border-radius:16px;padding:20px}.side-box h3{font:800 .9rem "Manrope";letter-spacing:.08em;text-transform:uppercase;margin-bottom:13px}.toc{list-style:none;max-height:66vh;overflow:auto;padding-right:5px}.toc a{display:block;padding:8px 10px;border-left:2px solid transparent;color:var(--muted);font-size:.82rem;line-height:1.3}.toc a:hover,.toc a.active{color:var(--indigo);border-left-color:var(--indigo);background:var(--soft);border-radius:0 8px 8px 0}.side-note{margin-top:14px;padding:14px;background:#f7f8fb;border-radius:11px;color:var(--muted);font-size:.78rem}.side-note b{color:var(--ink)}
.article{min-width:0}.intro{background:#fff;border:1px solid var(--line);padding:clamp(28px,4vw,48px);border-radius:20px;margin-bottom:38px}.intro p{font:400 1.16rem/1.78 "Newsreader";color:var(--body);margin-bottom:1em}.intro p:first-of-type:first-letter{float:left;font:800 4rem/.78 "Manrope";color:var(--indigo);padding:10px 12px 0 0}.editor-note{margin-top:25px;background:var(--dark);color:#d9ddea;padding:22px 24px;border-radius:14px}.editor-note strong{color:#fff}.review{scroll-margin-top:100px;margin:0 0 64px}.review-heading{display:grid;grid-template-columns:auto 1fr;gap:17px;align-items:center;margin-bottom:20px}.big-rank{width:52px;height:52px;border-radius:15px;background:var(--dark);color:#fff;display:grid;place-items:center;font:800 1.35rem "Manrope"}.review h2{font:800 clamp(1.8rem,3vw,2.6rem)/1.1 "Manrope";letter-spacing:-.04em}.review-by{color:var(--muted);font-size:.87rem;margin-top:5px}.review-shell{background:#fff;border:1px solid var(--line);border-radius:20px;overflow:hidden;box-shadow:0 8px 28px rgba(25,31,49,.055)}.review-top{display:grid;grid-template-columns:250px 1fr}.art-panel{min-height:340px;color:#fff;padding:26px;display:flex;flex-direction:column;justify-content:space-between;position:relative;overflow:hidden}.art-panel:before,.art-panel:after{content:"";position:absolute;border:28px solid rgba(255,255,255,.14);border-radius:50%}.art-panel:before{width:180px;height:180px;right:-75px;top:-60px}.art-panel:after{width:90px;height:90px;left:-42px;bottom:-38px}.art-panel .label{position:relative;z-index:1;font-size:.73rem;letter-spacing:.11em;font-weight:800;text-transform:uppercase}.art-panel .visual-title{position:relative;z-index:1;font:800 2.15rem/.98 "Manrope";letter-spacing:-.05em}.review-copy{padding:32px}.bestfor{display:inline-flex;background:var(--soft);color:var(--indigo);font-weight:800;font-size:.78rem;text-transform:uppercase;letter-spacing:.05em;padding:7px 11px;border-radius:7px}.review-copy>p{font:400 1.08rem/1.75 "Newsreader";color:var(--body);margin:20px 0}.fact-grid{display:grid;grid-template-columns:repeat(2,1fr);gap:1px;background:var(--line);border:1px solid var(--line);border-radius:12px;overflow:hidden}.fact{background:#fff;padding:14px}.fact small{display:block;color:var(--muted);font-size:.72rem;text-transform:uppercase;font-weight:800;letter-spacing:.06em}.fact b{font-size:.9rem}.review-bottom{border-top:1px solid var(--line);padding:28px 32px;display:grid;grid-template-columns:1fr 1fr;gap:24px}.review-bottom h4{font:800 .95rem "Manrope";margin-bottom:9px}.review-bottom ul{padding-left:18px;color:#565d6d;font-size:.9rem}.review-bottom li+li{margin-top:7px}.verdict{background:#f7f8fb;border-radius:12px;padding:18px;color:#555d6c;font-size:.9rem}.verdict b{display:block;color:var(--ink);margin-bottom:6px}.review-actions{display:flex;flex-wrap:wrap;gap:10px;margin-top:18px}.btn{border:0;border-radius:10px;padding:11px 16px;font-weight:800;font-size:.82rem}.btn.primary{background:var(--indigo);color:#fff}.btn.primary:hover{background:var(--indigo-2)}.btn.ghost{background:#f1f3f7;color:#414858}.btn.ghost:hover{background:#e7eaf0}
.method{background:linear-gradient(145deg,#171c2c,#0e111b);color:#fff;border-radius:22px;padding:clamp(30px,5vw,52px);margin:10px 0 48px}.method h2{font:800 clamp(2rem,3.5vw,3rem) "Manrope";letter-spacing:-.04em}.method>p{color:#b9c0cf;max-width:730px;margin:12px 0 25px}.method-grid{display:grid;grid-template-columns:repeat(4,1fr);gap:12px}.method-card{background:rgba(255,255,255,.07);border:1px solid rgba(255,255,255,.1);padding:19px;border-radius:12px}.method-card b{display:block;margin:8px 0 6px}.method-card span{color:#b9c0cf;font-size:.82rem}
.sources-wrap{margin:0 0 60px}.sources-details{background:#fff;border:1px solid var(--line);border-radius:18px;overflow:hidden}.sources-details summary{list-style:none;padding:22px 24px;cursor:pointer;display:flex;align-items:center;justify-content:space-between;font:800 1rem "Manrope"}.sources-details summary::-webkit-details-marker{display:none}.sources-details summary:after{content:"＋";font-size:1.35rem;color:var(--indigo);transition:.2s}.sources-details[open] summary:after{content:"−"}.source-summary{display:flex;align-items:center;gap:12px}.source-icon{width:36px;height:36px;background:var(--soft);color:var(--indigo);border-radius:10px;display:grid;place-items:center}.source-summary small{display:block;color:var(--muted);font:500 .76rem "DM Sans";margin-top:2px}.source-list{border-top:1px solid var(--line);padding:10px 24px 24px;display:grid;grid-template-columns:1fr 1fr;gap:8px 18px}.source-row{padding:13px 0;border-bottom:1px solid #eef0f4;display:grid;grid-template-columns:30px 1fr auto;gap:10px;align-items:center}.source-no{width:27px;height:27px;border-radius:8px;background:#f1f3f7;display:grid;place-items:center;font-weight:800;font-size:.75rem}.source-row strong{display:block;font-size:.88rem}.source-row span{display:block;color:var(--muted);font-size:.73rem}.source-row a{color:var(--indigo);font-weight:800;font-size:.76rem}.source-row a:hover{text-decoration:underline}
.backtop{position:fixed;right:22px;bottom:22px;width:46px;height:46px;border-radius:50%;border:0;background:var(--dark);color:#fff;box-shadow:var(--shadow);opacity:0;pointer-events:none;transform:translateY(8px);transition:.2s}.backtop.show{opacity:1;pointer-events:auto;transform:none}
.footer{background:var(--dark);color:#b7bdca;padding:52px 26px 28px}.footer-in{max-width:var(--max);margin:auto;display:grid;grid-template-columns:1.5fr 1fr 1fr 1fr;gap:36px}.footer h4{color:#fff;margin-bottom:13px}.footer p,.footer a{font-size:.84rem}.footer a{display:block;margin:8px 0}.footer a:hover{color:#fff}.copyright{max-width:var(--max);margin:36px auto 0;padding-top:20px;border-top:1px solid #2c3240;font-size:.76rem;display:flex;justify-content:space-between;gap:16px;flex-wrap:wrap}
.mobile-toc{display:none;background:#fff;border:1px solid var(--line);border-radius:13px;margin-bottom:24px}.mobile-toc summary{padding:15px 18px;font-weight:800;cursor:pointer}.mobile-toc .toc{padding:0 16px 16px;max-height:300px}
@media(max-width:1100px){.card-grid{grid-template-columns:repeat(3,1fr)}.hero-panel{grid-template-columns:1fr 420px}.main-layout{grid-template-columns:235px minmax(0,1fr);gap:34px}.review-top{grid-template-columns:210px 1fr}.method-grid{grid-template-columns:repeat(2,1fr)}}
@media(max-width:850px){.navlinks,.nav-search{display:none}.mobile-menu{display:block}.hero-panel{grid-template-columns:1fr}.hero-art{min-height:390px}.trustbar{grid-template-columns:1fr 1fr}.card-grid{grid-template-columns:repeat(2,1fr)}.main-layout{display:block}.sidebar{display:none}.mobile-toc{display:block}.review-top{grid-template-columns:1fr}.art-panel{min-height:270px}.review-bottom{grid-template-columns:1fr}.source-list{grid-template-columns:1fr}.footer-in{grid-template-columns:1fr 1fr}}
@media(max-width:560px){.hero,.explorer,.main-layout{padding-left:16px;padding-right:16px}.hero-copy{padding:38px 26px}.hero h1{font-size:2.65rem}.hero-art{min-height:320px}.cover{width:46%}.cover strong{font-size:1.2rem}.trustbar{padding:0 16px;grid-template-columns:1fr}.card-grid{grid-template-columns:1fr}.toolbar{align-items:stretch}.filters{overflow:auto;flex-wrap:nowrap;padding-bottom:4px}.filter{white-space:nowrap}.fact-grid{grid-template-columns:1fr}.review-copy,.review-bottom{padding:24px}.method-grid{grid-template-columns:1fr}.footer-in{grid-template-columns:1fr}.nav{padding:0 16px}}
@media(prefers-reduced-motion:reduce){*{scroll-behavior:auto!important;transition:none!important}}
</style>
</head>
<body>
<div class="progress"><span id="progressBar"></span></div>
<header class="site-header">
  <div class="nav">
    <a class="logo" href="#top">Manga<b>Vox</b></a>
    <nav class="navlinks"><a href="#discover">Discover</a><a href="#reviews">Reviews</a><a href="#methodology">Methodology</a><a href="#sources">Sources</a></nav>
    <input id="headerSearch" class="nav-search" type="search" placeholder="Search this guide…" aria-label="Search manga in this guide">
    <button class="mobile-menu" onclick="document.getElementById('mobileContents').open=true;location.hash='reviews'">Contents</button>
  </div>
</header>
<main id="top">
  <section class="hero">
    <div class="crumbs"><a href="#top">Home</a> &nbsp;/&nbsp; Editorial Guides &nbsp;/&nbsp; ${esc(data.category || 'Research')}</div>
    <div class="hero-panel">
      <div class="hero-copy">
        <div class="eyebrow">MangaVox definitive guide</div>
        <h1>${title}</h1>
        <p>${esc(data.introParagraphs?.[0] || 'A connected, filterable reading guide built for discovery—not endless scrolling. Explore every recommendation, jump directly to its full review, and open the source drawer only when you need verification.')}</p>
        <div class="hero-meta"><div class="avatar"></div><div><b>${esc(data.author || 'MangaVox Editorial Board')}</b><br>Updated ${date} · ${esc(data.readTime || '10 min')} guide</div></div>
      </div>
      <div class="hero-art" aria-label="Abstract manga cover collage" ${heroArtStyle}>
        <div class="cover-stack">
          <div class="cover c1" ${c1_style}><small>${esc(sec1_category.toUpperCase())}</small><strong>${esc(sec1_title)}</strong></div>
          <div class="cover c2" ${c2_style}><small>${esc(sec2_category.toUpperCase())}</small><strong>${esc(sec2_title)}</strong></div>
          <div class="cover c3" ${c3_style}><small>${esc(sec3_category.toUpperCase())}</small><strong>${esc(sec3_title)}</strong></div>
        </div>
      </div>
    </div>
  </section>

  <section class="trustbar">
    <div class="trust"><strong>${data.sections.length} titles</strong><span>Across various genres and demographics</span></div>
    <div class="trust"><strong>${data.sources ? data.sources.length : 0} official links</strong><span>Every recommendation connects to a publisher page</span></div>
    <div class="trust"><strong>Live filters</strong><span>Search by title, creator, genre or reading mood</span></div>
    <div class="trust"><strong>Closed sources</strong><span>Verification stays out of the way until requested</span></div>
  </section>

  <section class="explorer" id="discover">
    <div class="section-kicker">Find your next series</div>
    <h2 class="section-title">Explore the complete shortlist</h2>
    <p class="section-sub">Use the filters to narrow the list, then open any card to jump to its detailed editorial breakdown below.</p>
    <div class="toolbar">
      <div class="searchbox"><input id="directorySearch" type="search" placeholder="Search title, creator, genre or mood…"></div>
      <div class="filters" id="filters"></div>
    </div>
    <div class="result-count" id="resultCount"></div>
    <div class="card-grid" id="cardGrid"></div>
    <div class="empty" id="emptyState">No titles match that search. Try another genre or remove a keyword.</div>
  </section>

  <section class="main-layout" id="reviews">
    <aside class="sidebar"><div class="side-box"><h3>On this page</h3><ul class="toc" id="desktopToc"></ul><div class="side-note"><b>Connected navigation:</b> every card, title and source number links back to the same review entry.</div></div></aside>
    <article class="article">
      <details class="mobile-toc" id="mobileContents"><summary>Open table of contents</summary><ul class="toc" id="mobileToc"></ul></details>
      <section class="intro" id="introduction">
        ${(data.introParagraphs || []).map((p) => `<p>${esc(p)}</p>`).join('\n')}
        <div class="editor-note"><strong>How to use this page:</strong> browse by genre above, use the sticky contents list for direct navigation, and open the source drawer at the bottom for official publisher references. The source section is intentionally closed by default.</div>
      </section>
      <div id="reviewContainer"></div>

      <section class="method" id="methodology">
        <div class="section-kicker" style="color:#bcb6ff">Editorial framework</div>
        <h2>How MangaVox evaluates a series</h2>
        <p>This page is an editorial recommendation guide rather than a mathematical popularity chart. Every title is assessed using the same four practical reader-focused criteria.</p>
        <div class="method-grid">
          <div class="method-card"><div>01</div><b>Opening strength</b><span>Does the first volume communicate a compelling promise without demanding excessive patience?</span></div>
          <div class="method-card"><div>02</div><b>Visual clarity</b><span>Are action, emotion and pacing readable across both print pages and mobile screens?</span></div>
          <div class="method-card"><div>03</div><b>Distinct identity</b><span>Does the series have a tone, world or character dynamic that feels recognisably its own?</span></div>
          <div class="method-card"><div>04</div><b>Reader momentum</b><span>Does each arc create enough curiosity, emotional investment or thematic depth to continue?</span></div>
        </div>
      </section>

      <section class="sources-wrap" id="sources">
        <details class="sources-details" id="sourceDrawer">
          <summary><span class="source-summary"><span class="source-icon">↗</span><span>Sources and further reading<small>Closed by default · click to view all ${data.sources.length} references</small></span></span></summary>
          <div class="source-list" id="sourceList">
            ${sourcesHtml}
          </div>
        </details>
      </section>
    </article>
  </section>
</main>
<button class="backtop" id="backTop" aria-label="Back to top">↑</button>
<footer class="footer">
  <div class="footer-in">
    <div><div class="logo" style="color:#fff;margin-bottom:12px">Manga<b>Vox</b></div><p>Long-form manga discovery guides with connected navigation, transparent methodology and publisher-first sources.</p></div>
    <div><h4>Explore</h4><a href="#discover">All recommendations</a><a href="#reviews">Full reviews</a><a href="#methodology">Methodology</a></div>
    <div><h4>Genres</h4><a href="#discover">Action</a><a href="#discover">Fantasy</a><a href="#discover">Romance</a></div>
    <div><h4>Trust</h4><a href="#sources">Official sources</a><a href="#methodology">Editorial policy</a><a href="#top">Accessibility</a></div>
  </div>
  <div class="copyright"><span>© 2026 MangaVox Media. Editorial prototype.</span><span>Series names belong to their respective creators and publishers.</span></div>
</footer>
<script>
const manga = ${JSON.stringify(mappedManga, null, 2)};
const genres = ["All", ...new Set(manga.map(m => m.genre))];
let activeGenre = "All";
const filters = document.getElementById('filters');
const grid = document.getElementById('cardGrid');
const reviews = document.getElementById('reviewContainer');
const desktopToc = document.getElementById('desktopToc');
const mobileToc = document.getElementById('mobileToc');
const sources = document.getElementById('sourceList');

function safe(s) {
  return String(s || '').replace(/[&<>'"]/g, c => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', "'": '&#39;', '"': '&quot;'
  }[c] || c));
}

filters.innerHTML = genres.map(g => {
  const activeClass = g === 'All' ? 'active' : '';
  return '<button class="filter ' + activeClass + '" data-genre="' + safe(g) + '">' + safe(g) + '</button>';
}).join('');

function renderDirectory() {
  const q = document.getElementById('directorySearch').value.trim().toLowerCase();
  const list = manga.filter(m => 
    (activeGenre === 'All' || m.genre === activeGenre) &&
    [m.title, m.creator, m.genre, m.tone, m.best].join(' ').toLowerCase().includes(q)
  );
  document.getElementById('resultCount').textContent = 'Showing ' + list.length + ' of ' + manga.length + ' recommendations';
  
  grid.innerHTML = list.map(m => {
    const rankStr = String(m.rank).padStart(2, '0');
    const imageTag = m.image 
      ? '<img src="' + safe(m.image) + '" alt="" style="position:absolute;inset:0;width:100%;height:100%;object-fit:cover;opacity:0.3;mix-blend-mode:luminosity;" referrerpolicy="no-referrer">'
      : '';
    
    return '<article class="m-card">' +
      '<div class="mini-cover" style="background:' + m.grad + '">' +
        imageTag +
        '<span class="rank" style="position:relative;z-index:2;">' + rankStr + '</span>' +
        '<span class="genre" style="position:relative;z-index:2;">' + safe(m.genre) + '</span>' +
      '</div>' +
      '<div class="card-body">' +
        '<h3>' + safe(m.title) + '</h3>' +
        '<div class="creator">' + safe(m.creator) + '</div>' +
        '<p class="hook">' + safe(m.hook) + '</p>' +
        '<a class="card-link" href="#' + m.slug + '"><span>Open full review</span><span>↓</span></a>' +
      '</div>' +
    '</article>';
  }).join('');
  
  document.getElementById('emptyState').style.display = list.length ? 'none' : 'block';
}

filters.addEventListener('click', e => {
  if (!e.target.matches('.filter')) return;
  activeGenre = e.target.dataset.genre;
  document.querySelectorAll('.filter').forEach(b => b.classList.toggle('active', b === e.target));
  renderDirectory();
});

document.getElementById('directorySearch').addEventListener('input', renderDirectory);
document.getElementById('headerSearch').addEventListener('input', e => {
  document.getElementById('directorySearch').value = e.target.value;
  renderDirectory();
  location.hash = 'discover';
});

reviews.innerHTML = manga.map(m => {
  const artStyle = m.image 
    ? "background: linear-gradient(145deg, rgba(17, 21, 34, 0.78), rgba(17, 21, 34, 0.95)), url('" + m.image + "') center/cover;" 
    : "background:" + m.grad + ";";
  
  const strengthsHtml = m.strengths.map(s => '<li>' + safe(s) + '</li>').join('');
  
  return '<section class="review" id="' + m.slug + '" data-title="' + safe(m.title) + '">' +
    '<div class="review-heading">' +
      '<div class="big-rank">' + m.rank + '</div>' +
      '<div><h2>' + safe(m.title) + '</h2><div class="review-by">By ' + safe(m.creator) + ' · ' + safe(m.publisher) + '</div></div>' +
    '</div>' +
    '<div class="review-shell">' +
      '<div class="review-top">' +
        '<div class="art-panel" style="' + artStyle + '">' +
          '<span class="label">MangaVox #' + m.rank + '</span>' +
          '<span class="visual-title">' + safe(m.title) + '</span>' +
        '</div>' +
        '<div class="review-copy">' +
          '<span class="bestfor">Best for: ' + safe(m.best) + '</span>' +
          '<p>' + safe(m.overview) + '</p>' +
          '<div class="fact-grid">' +
            '<div class="fact"><small>Primary genre</small><b>' + safe(m.genre) + '</b></div>' +
            '<div class="fact"><small>Reading mood</small><b>' + safe(m.tone) + '</b></div>' +
            '<div class="fact"><small>Series status</small><b>' + safe(m.status) + '</b></div>' +
            '<div class="fact"><small>Pacing</small><b>' + safe(m.time) + '</b></div>' +
          '</div>' +
          '<div class="review-actions">' +
            '<a class="btn primary" href="' + m.url + '" target="_blank" rel="noopener">Official publisher page ↗</a>' +
            '<button class="btn ghost source-jump" data-source="source-' + m.rank + '">View source #' + m.rank + '</button>' +
          '</div>' +
        '</div>' +
      '</div>' +
      '<div class="review-bottom">' +
        '<div><h4>Why it stands out</h4><ul>' + strengthsHtml + '</ul></div>' +
        '<div class="verdict"><b>MangaVox verdict</b>' + safe(m.verdict) + '</div>' +
      '</div>' +
    '</div>' +
  '</section>';
}).join('');

const tocItems = '<li><a href="#introduction">Introduction</a></li>' +
  manga.map(m => '<li><a href="#' + m.slug + '">' + m.rank + '. ' + safe(m.title) + '</a></li>').join('') +
  '<li><a href="#methodology">Methodology</a></li><li><a href="#sources">Sources</a></li>';

desktopToc.innerHTML = tocItems;
mobileToc.innerHTML = tocItems;

sources.innerHTML = manga.map(m => 
  '<div class="source-row" id="source-' + m.rank + '">' +
    '<div class="source-no">' + m.rank + '</div>' +
    '<div><strong>' + safe(m.title) + '</strong><span>' + safe(m.source) + '</span></div>' +
    '<a href="' + m.url + '" target="_blank" rel="noopener">Open ↗</a>' +
  '</div>'
).join('');

document.addEventListener('click', e => {
  const b = e.target.closest('.source-jump');
  if (!b) return;
  const drawer = document.getElementById('sourceDrawer');
  drawer.open = true;
  setTimeout(() => {
    const targetEl = document.getElementById(b.dataset.source);
    if (targetEl) targetEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, 60);
});

const observed = [
  document.getElementById('introduction'),
  ...document.querySelectorAll('.review'),
  document.getElementById('methodology'),
  document.getElementById('sources')
].filter(Boolean);

const observer = new IntersectionObserver(entries => {
  entries.forEach(en => {
    if (en.isIntersecting) {
      document.querySelectorAll('.toc a').forEach(a => {
        a.classList.toggle('active', a.getAttribute('href') === '#' + en.target.id);
      });
    }
  });
}, { rootMargin: '-15% 0px -72% 0px' });

observed.forEach(el => observer.observe(el));

const progress = document.getElementById('progressBar');
const topBtn = document.getElementById('backTop');

window.addEventListener('scroll', () => {
  const h = document.documentElement.scrollHeight - window.innerHeight;
  if (progress) {
    progress.style.width = (h ? (window.scrollY / h * 100) : 0) + '%';
  }
  if (topBtn) {
    topBtn.classList.toggle('show', window.scrollY > 700);
  }
});

if (topBtn) {
  topBtn.addEventListener('click', () => {
    window.scrollTo({ top: 0, behavior: 'smooth' });
  });
}

renderDirectory();
</script>
</body>
</html>`
}
