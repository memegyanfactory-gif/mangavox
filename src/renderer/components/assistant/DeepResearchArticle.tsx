import React, { useEffect, useRef } from 'react'

export interface DeepResearchData {
  heroTitle: string
  heroImage: string
  category: string
  author: string
  publishedDate: string
  readTime: string
  authorBio: string
  introParagraphs: string[]
  sections: Array<{
    title: string
    image: string
    imageCredit: string
    paragraphs: string[]
    /** 0–5 stars (halves allowed) for rankable works (manga, movies, products…). */
    rating?: number
    /** Compact facts line, e.g. "Eiichiro Oda · 1997 · Ongoing · 1100+ chapters". */
    meta?: string
  }>
  keyTakeaways?: string[]
  trendingSidebar: Array<{
    title: string
    url: string
    image: string
  }>
  recommendedCards: Array<{
    title: string
    url: string
    image: string
  }>
  sources: Array<{
    title: string
    url: string
    domain: string
  }>
  tags: string[]
}

export function StarRating({ value }: { value: number }) {
  const v = Math.max(0, Math.min(5, value))
  const stars = [1, 2, 3, 4, 5].map((i) => (v >= i ? '★' : v >= i - 0.5 ? '⯨' : '☆'))
  return (
    <span className="stars" title={`${v} / 5`}>
      <span aria-hidden>{stars.join('')}</span>
      <span className="stars-n">{v.toFixed(1)}</span>
    </span>
  )
}

export function DeepResearchArticle({ data }: { data: DeepResearchData }) {
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const reduce = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches
    const els = rootRef.current?.querySelectorAll('.reveal')
    if (!els || els.length === 0) return
    
    if (reduce || !('IntersectionObserver' in window)) {
      els.forEach(e => e.classList.add('in'))
      return
    }
    
    const io = new IntersectionObserver((entries) => {
      entries.forEach(en => {
        if (en.isIntersecting) {
          en.target.classList.add('in')
          io.unobserve(en.target)
        }
      })
    }, { threshold: 0.12 })
    
    els.forEach(e => io.observe(e))
    return () => io.disconnect()
  }, [])

  return (
    <div ref={rootRef} className="deep-research-article" style={{ backgroundColor: '#101012', color: '#f4f4f6', fontFamily: '"Source Serif 4", Georgia, serif', fontSize: '18px', lineHeight: 1.7, paddingBottom: '60px' }}>
      <style dangerouslySetInnerHTML={{ __html: `
        .deep-research-article { overflow-x: hidden; -webkit-font-smoothing: antialiased; }
        .deep-research-article img { display: block; max-width: 100%; }
        .deep-research-article a { color: #cfcfe6; text-decoration: none; }
        .deep-research-article h1, .deep-research-article h2, .deep-research-article h3, .font-head, .brand, .chip, .badge { font-family: "Oswald", Impact, sans-serif; letter-spacing: .01em; }
        
        .hero { position: relative; height: clamp(360px, 52vh, 560px); overflow: hidden; }
        .hero img { width: 100%; height: 100%; object-fit: cover; }
        .hero .grad { position: absolute; inset: 0; background: linear-gradient(0deg, #101012 4%, rgba(16,16,18,.55) 45%, rgba(16,16,18,.12) 100%); }
        .hero .cap { position: absolute; left: 0; bottom: 0; padding: clamp(20px, 4vw, 46px); max-width: 1180px; }
        .chip { display: inline-block; background: #e3262e; color: #fff; font-weight: 600; font-size: .72rem; text-transform: uppercase; letter-spacing: .08em; padding: 5px 11px; border-radius: 4px; }
        .hero h1 { margin-top: 14px; font-weight: 700; font-size: clamp(2.2rem, 6vw, 4.2rem); line-height: 1.02; }
        
        .byline { max-width: 1180px; margin: 0 auto; padding: 20px 22px; display: flex; gap: 14px; align-items: flex-start; border-bottom: 1px solid #2a2a2f; }
        .byline img { width: 44px; height: 44px; border-radius: 50%; flex: none; }
        .byline .who { font-family: "Oswald", sans-serif; font-size: .95rem; }
        .byline .who b { font-weight: 600; }
        .byline .who .meta { color: #a6a6ad; font-weight: 500; }
        .byline .bio { font-size: .82rem; color: #a6a6ad; margin-top: 3px; font-family: "Source Serif 4", serif; }
        
        .wrap { max-width: 1180px; margin: 0 auto; padding: 34px 22px 10px; display: grid; grid-template-columns: minmax(0, 1fr) 320px; gap: 44px; }
        @media (max-width: 960px) { .wrap { grid-template-columns: 1fr; gap: 30px; } }
        
        .deep-research-article p { margin: 0 0 1.15em; }
        .deep-research-article p em { font-style: italic; }
        .intro > p:first-of-type::first-letter { font-family: "Oswald", sans-serif; font-weight: 700; float: left; font-size: 3.4rem; line-height: .8; padding: 6px 10px 0 0; color: #7c5cff; }
        h2.sec { font-size: clamp(1.5rem, 3vw, 2.1rem); font-weight: 700; margin: 38px 0 16px; line-height: 1.12; }
        h2.sec .n { color: #e3262e; }
        
        figure.sec-fig { position: relative; margin: 0 0 14px; border-radius: 10px; overflow: hidden; box-shadow: 0 10px 30px rgba(0,0,0,.45); background: #1d1d20; }
        figure.sec-fig img { width: 100%; aspect-ratio: 16/9; object-fit: cover; }
        .credit { position: absolute; top: 8px; right: 8px; background: rgba(0,0,0,.6); color: #e9e9ef; font-size: .66rem; padding: 3px 8px; border-radius: 20px; backdrop-filter: blur(4px); }
        
        .stars { display: inline-flex; align-items: center; gap: 8px; color: #e3262e; font-size: 1.05rem; letter-spacing: 2px; }
        .stars-n { font-family: "Oswald", sans-serif; font-size: .82rem; color: #a6a6ad; letter-spacing: 0; }
        .sec-meta { display: flex; flex-wrap: wrap; align-items: center; gap: 14px; margin: -8px 0 14px; font-family: "Oswald", sans-serif; font-size: .85rem; color: #a6a6ad; }
        .takeaways { background: #1d1d20; border: 1px solid #2a2a2f; border-left: 4px solid #7c5cff; border-radius: 10px; padding: 20px 22px; margin: 30px 0 8px; }
        .takeaways h3 { font-family: "Oswald", sans-serif; font-weight: 700; text-transform: uppercase; font-size: 1rem; margin-bottom: 12px; }
        .takeaways li { list-style: none; padding: 5px 0 5px 26px; position: relative; font-size: .95rem; }
        .takeaways li::before { content: "✓"; position: absolute; left: 0; color: #7c5cff; font-weight: 700; }
        .tags { display: flex; flex-wrap: wrap; gap: 8px; margin: 26px 0 18px; }
        .tag { background: #1d1d20; border: 1px solid #2a2a2f; color: #f4f4f6; font-family: "Oswald", sans-serif; font-size: .78rem; padding: 6px 12px; border-radius: 20px; }
        
        aside .stick { position: sticky; top: 80px; }
        .side-h { display: flex; align-items: center; gap: 10px; font-family: "Oswald", sans-serif; font-weight: 700; text-transform: uppercase; font-size: 1.05rem; margin-bottom: 14px; }
        .side-h::before { content: ""; width: 14px; height: 14px; background: #e3262e; display: inline-block; }
        .trend { display: flex; gap: 12px; padding: 11px 0; border-bottom: 1px solid #2a2a2f; align-items: center; }
        .trend:hover .t-title { color: #7c5cff; }
        .trend img { width: 84px; height: 56px; object-fit: cover; border-radius: 6px; flex: none; }
        .t-title { font-family: "Oswald", sans-serif; font-weight: 500; font-size: .94rem; line-height: 1.2; transition: color .2s; }
        @media (max-width: 960px) { aside .stick { position: static; } }
        
        .reco { max-width: 1180px; margin: 24px auto 0; padding: 30px 22px; }
        .reco-h { display: flex; align-items: center; gap: 10px; font-family: "Oswald", sans-serif; font-weight: 700; text-transform: uppercase; font-size: 1.15rem; margin-bottom: 18px; }
        .reco-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 16px; }
        .rcard { position: relative; border-radius: 10px; overflow: hidden; aspect-ratio: 3/4; box-shadow: 0 8px 24px rgba(0,0,0,.45); display: block; }
        .rcard img { width: 100%; height: 100%; object-fit: cover; transition: transform .4s ease; }
        .rcard .rg { position: absolute; inset: 0; background: linear-gradient(0deg, rgba(0,0,0,.85) 8%, rgba(0,0,0,.05) 60%); }
        .rcard .rt { position: absolute; left: 14px; right: 14px; bottom: 14px; font-family: "Oswald", sans-serif; font-weight: 600; font-size: 1.02rem; line-height: 1.15; color: #fff; }
        .rcard:hover { transform: translateY(-5px); }
        .rcard:hover img { transform: scale(1.06); }
        
        .sources { max-width: 1180px; margin: 0 auto; padding: 26px 22px; }
        .sources h2 { font-family: "Oswald", sans-serif; font-weight: 700; text-transform: uppercase; font-size: 1.1rem; margin-bottom: 14px; }
        .src { display: flex; align-items: center; gap: 10px; padding: 9px 0; border-bottom: 1px solid #2a2a2f; font-size: .92rem; }
        .src img { width: 18px; height: 18px; border-radius: 4px; }
        
        .reveal { opacity: 0; transform: translateY(16px); transition: opacity .5s ease, transform .5s ease; }
        .reveal.in { opacity: 1; transform: none; }
        @media (prefers-reduced-motion: reduce) {
          .reveal { opacity: 1; transform: none; transition: none; }
          .rcard:hover, .rcard:hover img { transform: none; }
        }
      ` }} />
      
      <section className="hero">
        <img src={data.heroImage || "https://loremflickr.com/1700/820/" + encodeURIComponent(data.heroTitle) + "?lock=1"} alt={data.heroTitle} />
        <div className="grad"></div>
        <div className="cap">
          <span className="chip">{data.category || 'ARTICLE'}</span>
          <h1>{data.heroTitle}</h1>
        </div>
      </section>

      <div className="byline">
        <img src="https://i.pravatar.cc/96?u=mangavox" alt="author" />
        <div>
          <div className="who"><b>By {data.author || 'MangaVox Research'}</b> <span className="meta">· Published {data.publishedDate || new Date().toLocaleDateString()} · {data.readTime || '5 min'} read</span></div>
          <div className="bio">{data.authorBio || 'MangaVox Research compiles and cross-checks the most-discussed topics across the web.'}</div>
        </div>
      </div>

      <div className="wrap">
        <article>
          <div className="intro reveal" id="intro">
            {data.introParagraphs?.map((p, i) => <p key={i}>{p}</p>)}
          </div>

          {data.sections?.map((sec, i) => (
            <React.Fragment key={i}>
              <h2 className="sec reveal"><span className="n">{i + 1}.</span> {sec.title}</h2>
              {(typeof sec.rating === 'number' || sec.meta) && (
                <div className="sec-meta reveal">
                  {typeof sec.rating === 'number' && <StarRating value={sec.rating} />}
                  {sec.meta && <span>{sec.meta}</span>}
                </div>
              )}
              {sec.image && (
                <figure className="sec-fig reveal">
                  <img src={sec.image} alt={sec.title} loading="lazy" />
                  {sec.imageCredit && <span className="credit">Image via {sec.imageCredit}</span>}
                </figure>
              )}
              {sec.paragraphs?.map((p, j) => <p key={j} className="reveal">{p}</p>)}
            </React.Fragment>
          ))}

          {(data.keyTakeaways?.length ?? 0) > 0 && (
            <div className="takeaways reveal">
              <h3>Key Takeaways</h3>
              <ul>{data.keyTakeaways!.map((t, i) => <li key={i}>{t}</li>)}</ul>
            </div>
          )}

          {data.tags?.length > 0 && (
            <div className="tags reveal">
              {data.tags.map(t => <span key={t} className="tag">{t}</span>)}
            </div>
          )}
        </article>

        <aside>
          <div className="stick reveal">
            <div className="side-h">Trending Now</div>
            {data.trendingSidebar?.map((trend, i) => (
              <a key={i} className="trend" target="_blank" rel="noreferrer" href={trend.url || "https://duckduckgo.com/?q=" + encodeURIComponent(trend.title)}>
                <img src={trend.image} alt="" />
                <span className="t-title">{trend.title}</span>
              </a>
            ))}
          </div>
        </aside>
      </div>

      {data.recommendedCards?.length > 0 && (
        <section className="reco">
          <div className="reco-h">🔥 Recommended</div>
          <div className="reco-grid">
            {data.recommendedCards.map((rc, i) => (
              <a key={i} className="rcard reveal" target="_blank" rel="noreferrer" href={rc.url || "https://duckduckgo.com/?q=" + encodeURIComponent(rc.title)}>
                <img src={rc.image} alt="" />
                <div className="rg"></div>
                <div className="rt">{rc.title}</div>
              </a>
            ))}
          </div>
        </section>
      )}

      {data.sources?.length > 0 && (
        <section className="sources">
          <h2>Sources</h2>
          {data.sources.map((src, i) => (
            <a key={i} className="src" target="_blank" rel="noreferrer" href={src.url}>
              <img src={"https://www.google.com/s2/favicons?domain=" + src.domain + "&sz=32"} alt="" />
              {src.title}
            </a>
          ))}
        </section>
      )}
    </div>
  )
}
