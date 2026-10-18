import { useEffect, useMemo, useState } from 'react'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import type { ReadingHistory, LibraryCategory } from '../../shared/types'
import {
  BookOpen, Check, CheckSquare, Grid3X3, List, Play, Search, Star, Trash2, X, ArrowDownUp,
  FolderPlus, Pencil, Tag, Plus,
} from 'lucide-react'
import { EmptyState } from '../components/EmptyState'
import { useLibraryStore } from '../stores/library.store'
import { useUiStore } from '../stores/ui.store'
import { coverUrl } from '../utils/cover-url'

type LibraryTab = string // 'all' | 'favorites' | category id
type StatusFilter = 'all' | 'ongoing' | 'completed' | 'hiatus'

const DENSITY_KEY = 'mv.library.density'

export function Library() {
  const {
    manga, loading, fetchLibrary, viewMode, setViewMode, toggleFavorite,
    removeManyFromLibrary, clearLibrary,
  } = useLibraryStore()
  const navigate = useUiStore((state) => state.navigate)
  const [filter, setFilter] = useState('')
  const [sortBy, setSortBy] = useState<'title' | 'added_at' | 'last_read_at'>('added_at')
  const [tab, setTab] = useState<LibraryTab>('all')
  const [selecting, setSelecting] = useState(false)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [status, setStatus] = useState<StatusFilter>('all')
  const [asc, setAsc] = useState(false)
  const [categories, setCategories] = useState<LibraryCategory[]>([])
  const [catMap, setCatMap] = useState<Record<string, string[]>>({})
  const [menuFor, setMenuFor] = useState<string | null>(null)
  const [newCatName, setNewCatName] = useState('')
  const [history, setHistory] = useState<ReadingHistory[]>([])
  const [density, setDensity] = useState<number>(() => {
    try { return Number(localStorage.getItem(DENSITY_KEY)) || 150 } catch { return 150 }
  })
  const changeDensity = (v: number) => {
    setDensity(v)
    try { localStorage.setItem(DENSITY_KEY, String(v)) } catch { /* ignore */ }
  }

  useEffect(() => {
    invoke<ReadingHistory[]>(IPC.HISTORY_LIST).then((h) => setHistory(h || [])).catch(() => {})
  }, [])

  useEffect(() => {
    fetchLibrary()
  }, [fetchLibrary])

  const loadCategories = async () => {
    try {
      const [cats, map] = await Promise.all([
        invoke<LibraryCategory[]>(IPC.CATEGORY_LIST),
        invoke<Record<string, string[]>>(IPC.CATEGORY_MANGA_MAP),
      ])
      setCategories(cats || [])
      setCatMap(map || {})
    } catch { /* ignore */ }
  }
  useEffect(() => { loadCategories() }, [])

  const createCategory = async (name?: string): Promise<LibraryCategory | null> => {
    const n = (name ?? prompt('New category name'))?.trim()
    if (!n) return null
    try {
      const c = await invoke<LibraryCategory>(IPC.CATEGORY_CREATE, n)
      setCategories((prev) => [...prev, c])
      return c
    } catch { return null }
  }
  const renameCategory = async (c: LibraryCategory) => {
    const n = prompt('Rename category', c.name)?.trim()
    if (!n || n === c.name) return
    await invoke(IPC.CATEGORY_RENAME, c.id, n)
    setCategories((prev) => prev.map((x) => (x.id === c.id ? { ...x, name: n } : x)))
  }
  const deleteCategory = async (c: LibraryCategory) => {
    if (!confirm(`Delete category "${c.name}"? Manga in it stay in your library.`)) return
    await invoke(IPC.CATEGORY_DELETE, c.id)
    setTab('all')
    loadCategories()
  }
  const toggleMangaCategory = async (mangaId: string, catId: string) => {
    const cur = catMap[mangaId] ?? []
    const next = cur.includes(catId) ? cur.filter((x) => x !== catId) : [...cur, catId]
    setCatMap((prev) => ({ ...prev, [mangaId]: next }))
    await invoke(IPC.CATEGORY_SET_FOR_MANGA, mangaId, next)
  }

  const filtered = useMemo(() => {
    return manga
      .filter((item) => {
        if (tab === 'favorites' && !item.is_favorite) return false
        if (tab !== 'all' && tab !== 'favorites' && !(catMap[item.id] ?? []).includes(tab)) return false
        if (status !== 'all' && item.status !== status) return false
        if (!filter) return true
        const query = filter.toLowerCase()
        return item.title.toLowerCase().includes(query)
          || !!item.author?.toLowerCase().includes(query)
          || item.genres.some((genre) => genre.toLowerCase().includes(query))
      })
      .sort((a, b) => {
        const d = sortBy === 'title' ? a.title.localeCompare(b.title)
          : sortBy === 'last_read_at' ? (b.last_read_at ?? '').localeCompare(a.last_read_at ?? '')
          : b.added_at.localeCompare(a.added_at)
        return asc ? -d : d
      })
  }, [filter, manga, sortBy, tab, status, asc, catMap])

  const mangaIds = useMemo(() => new Set(manga.map((m) => m.id)), [manga])
  const continueReading = history.filter((h) => mangaIds.has(h.manga_id)).slice(0, 12)

  const favoriteCount = manga.filter((item) => item.is_favorite).length

  const toggleSelection = (id: string) => {
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const stopSelecting = () => {
    setSelecting(false)
    setSelected(new Set())
  }

  const deleteSelected = async () => {
    if (selected.size === 0) return
    if (!confirm(`Delete ${selected.size} selected manga from the library and remove their downloaded files?`)) return
    await removeManyFromLibrary([...selected])
    stopSelecting()
  }

  const resetLibrary = async () => {
    if (!confirm('Reset the entire library? This removes every manga, chapter record, history entry, and downloaded library file.')) return
    await clearLibrary()
    stopSelecting()
  }

  const handleOpen = (id: string) => {
    if (selecting) {
      toggleSelection(id)
      return
    }
    navigate('manga-detail', { mangaId: id })
  }

  const renderCategoryMenu = (mangaId: string) => (
    <>
      <button type="button" title="Categories"
        onClick={() => { setMenuFor(menuFor === mangaId ? null : mangaId); setNewCatName('') }}
        className="w-10 h-10 sm:w-6 sm:h-6 flex items-center justify-center rounded"
        style={{ color: (catMap[mangaId]?.length ?? 0) > 0 ? 'var(--accent)' : 'var(--text-3)' }}>
        <Tag size={13} />
      </button>
      {menuFor === mangaId && (
        <>
          <div className="fixed inset-0 z-30" onClick={() => setMenuFor(null)} />
          <div className="absolute right-0 bottom-10 sm:bottom-7 z-40 w-48 max-w-[80vw] rounded-lg p-1.5 shadow-xl"
            style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
            <div className="px-1.5 py-1 text-[10px] uppercase tracking-wider" style={{ color: 'var(--text-3)' }}>Categories</div>
            <div className="max-h-40 overflow-y-auto">
              {categories.length === 0 && <div className="px-1.5 py-1 text-[11px]" style={{ color: 'var(--text-3)' }}>No categories yet</div>}
              {categories.map((c) => {
                const on = (catMap[mangaId] ?? []).includes(c.id)
                return (
                  <button key={c.id} type="button" onClick={() => toggleMangaCategory(mangaId, c.id)}
                    className="w-full flex items-center gap-2 px-1.5 py-1 rounded text-xs text-left hover:brightness-125"
                    style={{ color: 'var(--text-1)' }}>
                    <span className="w-3.5 h-3.5 rounded-sm flex items-center justify-center shrink-0"
                      style={{ background: on ? 'var(--accent)' : 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
                      {on && <Check size={10} color="#fff" />}
                    </span>
                    <span className="truncate">{c.name}</span>
                  </button>
                )
              })}
            </div>
            <form className="flex items-center gap-1 mt-1 pt-1" style={{ borderTop: '1px solid var(--panel-border)' }}
              onSubmit={async (e) => {
                e.preventDefault()
                const c = await createCategory(newCatName)
                if (c) { setNewCatName(''); toggleMangaCategory(mangaId, c.id) }
              }}>
              <input value={newCatName} onChange={(e) => setNewCatName(e.target.value)} placeholder="New category"
                className="mv-input flex-1 min-w-0 h-6 text-[11px] px-1.5" />
              <button type="submit" className="mv-btn-icon" title="Create"><Plus size={12} /></button>
            </form>
          </div>
        </>
      )}
    </>
  )

  return (
    <div className="flex flex-col h-full">
      <div className="px-3 sm:px-5 pt-3 pb-1 flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        <div className="flex items-baseline gap-2 min-w-0">
          <h2 className="text-lg font-bold" style={{ color: 'var(--text-1)' }}>Library</h2>
          <span className="text-xs truncate" style={{ color: 'var(--text-3)' }}>
            {manga.length} series · {favoriteCount} favorite{favoriteCount === 1 ? '' : 's'}
          </span>
        </div>
        <div className="flex items-center gap-2">
          {selecting ? (
            <>
              <span className="text-xs" style={{ color: 'var(--text-3)' }}>{selected.size} selected</span>
              <button type="button" onClick={deleteSelected} disabled={selected.size === 0}
                className="h-10 sm:h-7 px-2.5 rounded-md flex items-center gap-1.5 text-xs font-semibold text-white disabled:opacity-40"
                style={{ background: '#dc2626' }}>
                <Trash2 size={12} /> Delete
              </button>
              <button type="button" onClick={stopSelecting} className="mv-btn-icon" title="Cancel selection"><X size={14} /></button>
            </>
          ) : (
            <>
              <button type="button" onClick={() => setSelecting(true)}
                className="h-10 sm:h-7 px-2.5 rounded-md flex items-center gap-1.5 text-xs"
                style={{ color: 'var(--text-2)', background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
                <CheckSquare size={12} /> Select
              </button>
              <button type="button" onClick={resetLibrary} disabled={manga.length === 0}
                className="h-10 sm:h-7 px-2.5 rounded-md flex items-center gap-1.5 text-xs disabled:opacity-40"
                style={{ color: '#f87171', background: 'var(--surface-2)', border: '1px solid rgba(248,113,113,.18)' }}>
                <Trash2 size={12} /> Reset
              </button>
            </>
          )}
        </div>
      </div>

      <div className="px-3 sm:px-5 py-2 flex flex-wrap items-center gap-2 min-w-0">
        <div className="mv-pill-tabs max-w-full overflow-x-auto scrollbar-none">
          <button type="button" onClick={() => setTab('all')} className={tab === 'all' ? 'is-active' : ''}>
            <BookOpen size={12} /> All
          </button>
          <button type="button" onClick={() => setTab('favorites')} className={tab === 'favorites' ? 'is-active' : ''}>
            <Star size={12} fill={tab === 'favorites' ? 'currentColor' : 'none'} /> Favorites
          </button>
          {categories.map((c) => (
            <button key={c.id} type="button" onClick={() => setTab(c.id)} className={tab === c.id ? 'is-active' : ''}>
              <Tag size={11} /> {c.name}
              <span className="opacity-60 ml-0.5">{manga.filter((m) => (catMap[m.id] ?? []).includes(c.id)).length}</span>
            </button>
          ))}
        </div>
        <button type="button" onClick={() => createCategory()} title="New category"
          className="h-10 w-10 sm:h-7 sm:w-auto justify-center px-2 rounded-md flex items-center gap-1 text-[11px]"
          style={{ color: 'var(--text-2)', background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
          <FolderPlus size={12} />
        </button>
        {tab !== 'all' && tab !== 'favorites' && (() => {
          const c = categories.find((x) => x.id === tab)
          if (!c) return null
          return (
            <>
              <button type="button" onClick={() => renameCategory(c)} title="Rename category" className="mv-btn-icon"><Pencil size={12} /></button>
              <button type="button" onClick={() => deleteCategory(c)} title="Delete category" className="mv-btn-icon" style={{ color: '#f87171' }}><Trash2 size={12} /></button>
            </>
          )
        })()}

        <div className="flex items-center gap-1 flex-wrap">
          {(['all', 'ongoing', 'completed', 'hiatus'] as StatusFilter[]).map((st) => (
            <button key={st} type="button" onClick={() => setStatus(st)}
              className="h-10 sm:h-7 px-2.5 rounded-full text-[11px] capitalize"
              style={{
                color: status === st ? '#fff' : 'var(--text-3)',
                background: status === st ? 'var(--accent)' : 'var(--surface-2)',
                border: '1px solid var(--panel-border)',
              }}>
              {st}
            </button>
          ))}
        </div>

        <div className="relative flex-1 min-w-[10rem] basis-full sm:basis-auto">
          <Search size={13} className="absolute left-3 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-3)' }} />
          <input type="text" placeholder="Filter title, author, or genre..." value={filter}
            onChange={(event) => setFilter(event.target.value)} className="mv-input w-full h-10 sm:h-8 pl-8 text-xs" />
        </div>

        <select value={sortBy} onChange={(event) => setSortBy(event.target.value as typeof sortBy)}
          className="h-10 sm:h-8 px-2 rounded-md text-xs outline-none cursor-pointer"
          style={{ color: 'var(--text-2)', background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
          <option value="added_at">Date added</option>
          <option value="title">Title</option>
          <option value="last_read_at">Last read</option>
        </select>
        <button type="button" onClick={() => setAsc((v) => !v)} title={asc ? 'Ascending' : 'Descending'}
          className="h-10 w-10 sm:h-8 sm:w-8 rounded-md flex items-center justify-center"
          style={{ color: asc ? 'var(--accent)' : 'var(--text-3)', background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
          <ArrowDownUp size={13} />
        </button>

        {viewMode === 'grid' && (
          <input type="range" min={110} max={240} step={10} value={density}
            onChange={(e) => changeDensity(Number(e.target.value))} title="Cover size"
            className="w-20 hidden sm:block" style={{ accentColor: 'var(--accent)' }} />
        )}

        <div className="flex h-10 sm:h-8 rounded-md overflow-hidden" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
          {(['grid', 'list'] as const).map((m) => (
            <button key={m} type="button" onClick={() => setViewMode(m)} className="px-3 sm:px-2.5"
              style={{ color: viewMode === m ? '#fff' : 'var(--text-3)', background: viewMode === m ? 'var(--accent-gradient)' : 'transparent' }}>
              {m === 'grid' ? <Grid3X3 size={13} /> : <List size={13} />}
            </button>
          ))}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto overflow-x-hidden px-3 sm:px-5 pb-6">
        {!loading && continueReading.length > 0 && !filter && tab === 'all' && (
          <div className="mb-4">
            <h3 className="text-[11px] font-semibold uppercase tracking-wider mb-2" style={{ color: 'var(--text-3)' }}>Continue reading</h3>
            <div className="flex gap-2 overflow-x-auto pb-1">
              {continueReading.map((h) => {
                const c = coverUrl(h.cover_url)
                return (
                  <button key={h.manga_id} type="button" className="shrink-0 flex items-center gap-2 p-1.5 pr-3 rounded-lg text-left w-56 max-w-[80vw]"
                    style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                    onClick={() => navigate('reader', { mangaId: h.manga_id, chapterId: h.chapter_id, sourceId: h.source_plugin, sourceChapterId: h.source_chapter_id })}>
                    <div className="w-9 h-12 rounded overflow-hidden shrink-0" style={{ background: 'var(--surface-3)' }}>
                      {c && <img src={c} alt="" className="w-full h-full object-cover" referrerPolicy="no-referrer" />}
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="text-xs font-medium truncate" style={{ color: 'var(--text-1)' }}>{h.manga_title}</div>
                      <div className="text-[10px] truncate" style={{ color: 'var(--text-3)' }}>
                        {h.chapter_number !== null ? `Ch. ${h.chapter_number}` : h.chapter_title || 'Chapter'} · p.{h.page + 1}
                      </div>
                    </div>
                    <Play size={12} style={{ color: 'var(--accent)' }} fill="currentColor" />
                  </button>
                )
              })}
            </div>
          </div>
        )}

        {loading && (
          <div className="grid gap-3" style={{ gridTemplateColumns: `repeat(auto-fill, minmax(min(${density}px, calc(50% - 6px)), 1fr))` }}>
            {Array.from({ length: 12 }).map((_, index) => (
              <div key={index}>
                <div className="aspect-[2/3] skeleton rounded-xl" />
                <div className="mt-2.5 skeleton h-4 w-3/4 rounded-md" />
              </div>
            ))}
          </div>
        )}

        {!loading && filtered.length === 0 && (
          manga.length === 0 ? (
            <EmptyState
              tone="accent"
              icon={<BookOpen size={28} strokeWidth={1.5} />}
              title="Your library is empty"
              description="Browse sources and add a manga to start building your library."
              action={{ label: 'Browse sources', onClick: () => navigate('browse') }}
            />
          ) : tab !== 'all' && tab !== 'favorites' ? (
            <EmptyState
              icon={<Tag size={28} strokeWidth={1.5} />}
              title="Empty category"
              description="Use the tag button under a manga card to add it to this category."
            />
          ) : tab === 'favorites' ? (
            <EmptyState
              icon={<Star size={28} strokeWidth={1.5} />}
              title="No favorites yet"
              description="Press the star on any manga and only starred titles will appear in this tab."
            />
          ) : (
            <EmptyState
              icon={<Search size={28} strokeWidth={1.5} />}
              title="No matches"
              description="Try a different search."
            />
          )
        )}

        {!loading && viewMode === 'grid' && filtered.length > 0 && (
          <div className="grid gap-3" style={{ gridTemplateColumns: `repeat(auto-fill, minmax(min(${density}px, calc(50% - 6px)), 1fr))` }}>
            {filtered.map((item, index) => {
              const cover = coverUrl(item.cover_path || item.cover_url || null)
              const isSelected = selected.has(item.id)
              return (
                <div key={item.id} className="group fade-up" style={{ animationDelay: `${Math.min(index, 20) * 25}ms` }}>
                  <button
                    type="button"
                    onClick={() => handleOpen(item.id)}
                    className="w-full text-left"
                  >
                    <div
                      className="aspect-[2/3] rounded-lg overflow-hidden relative transition-all group-hover:-translate-y-0.5"
                      style={{
                        background: 'var(--surface-2)',
                        border: isSelected ? '2px solid var(--accent)' : '1px solid var(--panel-border)',
                        boxShadow: isSelected ? '0 0 0 3px var(--accent-glow-soft)' : undefined,
                      }}
                    >
                      {cover ? (
                        <img src={cover} alt={item.title} className="w-full h-full object-cover" loading="lazy" referrerPolicy="no-referrer" />
                      ) : (
                        <span className="w-full h-full flex items-center justify-center text-4xl font-bold" style={{ color: 'var(--text-3)' }}>
                          {item.title.charAt(0)}
                        </span>
                      )}
                      {item.unread_count ? (
                        <span className="absolute top-1.5 right-1.5 px-1.5 py-0.5 rounded text-[10px] font-semibold text-white" style={{ background: 'var(--accent)' }} title={`${item.unread_count} unread`}>
                          {item.unread_count}
                        </span>
                      ) : null}
                      {selecting && (
                        <span
                          className="absolute top-2 left-2 w-6 h-6 rounded-full flex items-center justify-center"
                          style={{ color: isSelected ? '#fff' : 'var(--text-2)', background: isSelected ? 'var(--accent)' : 'rgba(0,0,0,.65)' }}
                        >
                          {isSelected && <Check size={14} />}
                        </span>
                      )}
                    </div>
                    <h3 className="text-xs font-medium mt-1.5 line-clamp-1" style={{ color: 'var(--text-2)' }}>{item.title}</h3>
                  </button>
                  <div className="mt-1 flex items-center justify-between relative">
                    <button
                      type="button"
                      onClick={() => toggleFavorite(item.id)}
                      className="flex items-center gap-1.5 min-h-[40px] sm:min-h-0 min-w-0 text-[11px] transition-colors"
                      style={{ color: item.is_favorite ? '#fbbf24' : 'var(--text-3)' }}
                    >
                      <Star size={13} fill={item.is_favorite ? 'currentColor' : 'none'} />
                      <span className="truncate">{item.is_favorite ? 'Favorite' : 'Add favorite'}</span>
                    </button>
                    {renderCategoryMenu(item.id)}
                  </div>
                </div>
              )
            })}
          </div>
        )}

        {!loading && viewMode === 'list' && filtered.length > 0 && (
          <div className="space-y-1.5">
            {filtered.map((item) => {
              const cover = coverUrl(item.cover_path || item.cover_url || null)
              const isSelected = selected.has(item.id)
              return (
                <div
                  key={item.id}
                  className="flex items-center gap-2 sm:gap-4 px-2 sm:px-3 py-1.5 rounded-lg transition-all min-w-0"
                  style={{
                    background: isSelected ? 'var(--accent-gradient-dim)' : 'var(--surface-2)',
                    border: isSelected ? '1px solid var(--panel-border-strong)' : '1px solid var(--panel-border)',
                  }}
                >
                  {selecting && (
                    <button
                      type="button"
                      onClick={() => toggleSelection(item.id)}
                      className="w-10 h-10 sm:w-6 sm:h-6 rounded-md flex items-center justify-center shrink-0"
                      style={{ color: isSelected ? '#fff' : 'var(--text-3)', background: isSelected ? 'var(--accent)' : 'var(--surface-4)' }}
                    >
                      {isSelected && <Check size={14} />}
                    </button>
                  )}
                  <button type="button" onClick={() => handleOpen(item.id)} className="flex items-center gap-3 flex-1 min-w-0 text-left">
                    <div className="w-8 h-11 rounded overflow-hidden shrink-0" style={{ background: 'var(--surface-3)' }}>
                      {cover ? (
                        <img src={cover} alt="" className="w-full h-full object-cover" referrerPolicy="no-referrer" />
                      ) : (
                        <span className="w-full h-full flex items-center justify-center font-bold" style={{ color: 'var(--text-3)' }}>{item.title.charAt(0)}</span>
                      )}
                    </div>
                    <div className="flex-1 min-w-0">
                      <h3 className="text-sm font-medium truncate" style={{ color: 'var(--text-1)' }}>{item.title}</h3>
                      <p className="text-xs truncate mt-0.5" style={{ color: 'var(--text-3)' }}>{item.author || 'Unknown author'}</p>
                    </div>
                  </button>
                  <button
                    type="button"
                    onClick={() => toggleFavorite(item.id)}
                    className="w-10 h-10 sm:w-8 sm:h-8 shrink-0 flex items-center justify-center"
                    style={{ color: item.is_favorite ? '#fbbf24' : 'var(--text-3)' }}
                    title={item.is_favorite ? 'Remove favorite' : 'Add favorite'}
                  >
                    <Star size={15} fill={item.is_favorite ? 'currentColor' : 'none'} />
                  </button>
                  {item.unread_count ? (
                    <span className="shrink-0 px-1.5 py-0.5 rounded text-[10px] font-semibold text-white whitespace-nowrap" style={{ background: 'var(--accent)' }}>{item.unread_count}<span className="hidden sm:inline"> unread</span></span>
                  ) : null}
                  <div className="relative shrink-0">{renderCategoryMenu(item.id)}</div>
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}
