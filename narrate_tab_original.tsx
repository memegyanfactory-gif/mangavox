          {activeTab === 'narrate' && selectedChapter && (
            <div className="flex flex-col h-full gap-4 max-w-4xl">
              {selectedManga && memory && (
                <button
                  onClick={() => setMemoryOpen(true)}
                  className="w-full rounded-lg px-4 py-2.5 flex items-center gap-3 text-left shrink-0 transition-all hover:brightness-110"
                  style={{
                    background: 'linear-gradient(90deg, rgba(var(--accent-rgb), .12), rgba(var(--secondary-rgb), .06))',
                    border: '1px solid var(--panel-border-strong)',
                  }}
                >
                  <Brain size={15} style={{ color: 'var(--accent-hover)' }} />
                  <div className="flex-1 min-w-0">
                    <div className="text-[11px] font-semibold" style={{ color: 'var(--text-2)' }}>Series memory</div>
                    <div className="text-[10px] truncate mt-0.5" style={{ color: 'var(--text-3)' }}>
                      {memory.characters.length} characters · {memory.facts.length} facts · {memory.chaptersNarrated.length} chapters remembered
                    </div>
                  </div>
                  <span className="text-[10px] font-semibold" style={{ color: 'var(--accent-hover)' }}>Review</span>
                </button>
              )}
              <div
                className="rounded-xl p-3 flex flex-wrap items-center gap-3"
                style={{ background: 'var(--surface-2)', border: '1px solid rgba(255,255,255,0.05)' }}
              >
                <div className="flex items-center gap-2 text-[11px] font-semibold text-zinc-300">
                  <LayoutGrid size={12} className="text-cyan-300" />
                  Generate pages
                </div>
                <input
                  type="number"
                  min={1}
                  max={pageCount || undefined}
                  value={narrateFromPage}
                  onChange={(e) => setNarrateFromPage(e.target.value)}
                  placeholder="From"
                  className="w-20 h-8 px-2 rounded-lg text-xs text-zinc-200 outline-none"
                  style={inputStyle}
                />
                <span className="text-zinc-600 text-xs">to</span>
                <input
                  type="number"
                  min={1}
                  max={pageCount || undefined}
                  value={narrateToPage}
                  onChange={(e) => setNarrateToPage(e.target.value)}
                  placeholder={pageCount ? `To ${pageCount}` : 'To'}
                  className="w-24 h-8 px-2 rounded-lg text-xs text-zinc-200 outline-none"
                  style={inputStyle}
                />
                <button
                  onClick={() => { setNarrateFromPage(''); setNarrateToPage('') }}
                  disabled={!narrateFromPage && !narrateToPage}
                  className="px-2 py-1 rounded-md text-[11px] text-zinc-400 hover:text-white disabled:opacity-30"
                  style={{ background: 'var(--surface-3)' }}
                >
                  Full
                </button>
                <div className="flex-1" />
                <div className="text-[11px] text-zinc-500">
                  {narrationRange
                    ? `Only pages ${narrationRange.from}-${narrationRange.to} will be generated`
                    : pageCount
                      ? `Full chapter · ${pageCount} pages`
                      : 'Page count loads after selecting a chapter'}
                </div>
              </div>

              {!finalNarration && !narrating && narrationPartial && (
                <div className="rounded-xl p-4 flex items-start gap-3"
                  style={{ background: 'rgba(245,158,11,0.08)', border: '1px solid rgba(245,158,11,0.3)' }}>
                  <AlertCircle size={16} className="text-amber-400 shrink-0 mt-0.5" />
                  <div className="flex-1 min-w-0">
                    <div className="text-xs font-semibold text-amber-300 mb-1">
                      Previous run stopped at batch {narrationPartial.next_batch_index}/{narrationPartial.total_batches}
                    </div>
                    <p className="text-[11px] text-zinc-400 leading-relaxed">
                      {narrationPartial.batches.length} batches were saved before the run ended. You can resume from
                      where it stopped or start over.
                    </p>
                    <div className="flex items-center gap-2 mt-3">
                      <button
                        onClick={() => handleNarrate(narrationPartial.provider as 'gemini' | 'ollama' | 'g0i', 'resume')}
                        className="flex items-center gap-2 px-4 py-2 rounded-lg text-xs font-semibold text-white transition-all hover:brightness-110"
                        style={{ background: 'linear-gradient(135deg, #06B6D4 0%, #6366F1 100%)' }}
                      >
                        <Eye size={13} /> Resume ({narrationPartial.provider})
                      </button>
                      <button
                        onClick={() => handleNarrate(narrationPartial.provider as 'gemini' | 'ollama' | 'g0i', 'fresh')}
                        className="flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-medium text-zinc-300 hover:text-white transition-all"
                        style={{ background: 'var(--surface-3)' }}
                      >
                        Start over
                      </button>
                    </div>
                  </div>
                </div>
              )}

              {!finalNarration && narrationPartial?.descriptions && narrationPartial.descriptions.length > 0 && (
                <div className="space-y-3">
                  <div className="flex items-center justify-between text-[11px] text-zinc-500">
                    <span className="font-semibold text-zinc-300">Generated page panels</span>
                    <span>{narrationPartial.descriptions.length} pages saved</span>
                  </div>
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                    {narrationPartial.descriptions.map((item) => {
                      const file = fileForPage(item.page - 1)
                      return (
                        <div
                          key={item.page}
                          className="overflow-hidden rounded-lg"
                          style={{ background: 'var(--surface-2)', border: '1px solid rgba(255,255,255,0.055)' }}
                        >
                          <div className="px-3 py-2 text-[10px] font-semibold text-zinc-400 border-b"
                            style={{ borderColor: 'rgba(255,255,255,0.05)' }}>
                            Page {item.page}
                          </div>
                          {file && (
                            <img
                              src={pageUrl(file)}
                              alt={`Page ${item.page}`}
                              className="w-full max-h-64 object-contain bg-black/30"
                            />
                          )}
                          <p className="p-3 text-xs leading-relaxed text-zinc-300 whitespace-pre-wrap">
                            {(item.text || 'Page was read, but no description was returned.')
                              // Strip internal focus-point data (---FOCUS--- and coordinate lines)
                              // that's meant for video generation, not for user display
                              .replace(/\n*---+\s*FOCUS\s*---+[\s\S]*/i, '')
                              .trim() || 'Page was read, but no description was returned.'}
                          </p>
                        </div>
                      )
                    })}
                  </div>
                </div>
              )}

              {!narration && !narrating && !narrationPartial && (
                <div className="flex flex-col items-center justify-center h-full text-zinc-600">
                  <Eye size={48} strokeWidth={1} className="mb-4 opacity-30 text-cyan-400" />
                  <p className="text-sm font-medium text-zinc-400">No narration yet</p>
                  <p className="text-xs mt-2 text-zinc-600 max-w-lg text-center leading-relaxed">
                    Sends every page to vision AI and writes one continuous prose narration —
                    even on pages with no text. Remembers characters across chapters.
                  </p>
                  
                  <div className="mt-6 flex flex-col items-center justify-center gap-3 w-full max-w-sm mx-auto">
                    {/* ── Primary Action Buttons ── */}
                    <div className="w-full grid grid-cols-1 gap-2.5">
                      {/* Gemini Button */}
                      <button
                        onClick={() => handleNarrate('gemini')}
                        disabled={!geminiConfigured}
                        onClickCapture={(e) => {
                          if (!geminiConfigured) {
                            e.preventDefault()
                            e.stopPropagation()
                            toast.info(
                              'Gemini needs an API key',
                              'Settings → AI Models → Google Gemini API. Free at aistudio.google.com/apikey.',
                            )
                          }
                        }}
                        className="w-full flex items-center gap-3 px-4 py-3 rounded-xl text-left transition-all hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
                        style={{
                          background: geminiConfigured
                            ? 'linear-gradient(135deg, rgba(6,182,212,0.15), rgba(99,102,241,0.1))'
                            : 'var(--surface-2)',
                          border: geminiConfigured
                            ? '1px solid rgba(6,182,212,0.25)'
                            : '1px solid rgba(255,255,255,0.05)',
                        }}
                      >
                        <div className="w-8 h-8 rounded-lg flex items-center justify-center shrink-0"
                          style={{ background: geminiConfigured ? 'linear-gradient(135deg, #06B6D4, #6366F1)' : 'var(--surface-3)' }}>
                          <Sparkles size={14} className="text-white" />
                        </div>
                        <div className="flex-1 min-w-0">
                          <div className="text-xs font-semibold text-zinc-200">Narrate with Gemini</div>
                          <div className="text-[10px] text-zinc-500 mt-0.5">
                            {geminiConfigured ? 'Cloud · Fast · Best quality' : 'Add free API key in Settings'}
                          </div>
                        </div>
                        {geminiConfigured && (
                          <span className="text-[9px] font-bold px-1.5 py-0.5 rounded-md bg-emerald-500/15 text-emerald-400 shrink-0">
                            READY
                          </span>
                        )}
                      </button>

                      {g0iConfigured && (
                        <button
                          onClick={() => handleNarrate('g0i')}
                          className="w-full flex items-center gap-3 px-4 py-3 rounded-xl text-left transition-all hover:brightness-110"
                          style={{
                            background: 'linear-gradient(135deg, rgba(96,165,250,0.12), rgba(37,99,235,0.08))',
                            border: '1px solid rgba(96,165,250,0.2)',
                          }}
                        >
                          <div className="w-8 h-8 rounded-lg flex items-center justify-center shrink-0"
                            style={{ background: 'linear-gradient(135deg, #60A5FA, #2563EB)' }}>
                            <Zap size={14} className="text-white" />
                          </div>
                          <div className="flex-1 min-w-0">
                            <div className="text-xs font-semibold text-zinc-200">Narrate with g0i.ai</div>
                            <div className="text-[10px] text-zinc-500 mt-0.5">Gemini / Claude / GPT — one key</div>
                          </div>
                        </button>
                      )}

                      {/* Local AI Button */}
                      <button
                        onClick={() => handleNarrate('ollama')}
                        className="w-full flex items-center gap-3 px-4 py-3 rounded-xl text-left transition-all hover:brightness-110"
                        style={{
                          background: ollamaOnline
                            ? 'linear-gradient(135deg, rgba(6,182,212,0.1), rgba(5,150,105,0.08))'
                            : 'var(--surface-2)',
                          border: ollamaOnline
                            ? '1px solid rgba(6,182,212,0.2)'
                            : '1px solid rgba(255,255,255,0.05)',
                        }}
                      >
                        <div className="w-8 h-8 rounded-lg flex items-center justify-center shrink-0"
                          style={{ background: ollamaOnline ? 'linear-gradient(135deg, #06B6D4, #059669)' : 'var(--surface-3)' }}>
                          <Cpu size={14} className="text-white" />
                        </div>
                        <div className="flex-1 min-w-0">
                          <div className="text-xs font-semibold text-zinc-200">
                            Narrate with Local AI
                          </div>
                          <div className="text-[10px] text-zinc-500 mt-0.5">
                            {localLlmType === 'lmstudio' ? 'LM Studio' : 'Ollama'} ·
                            {localLlmModel ? ` ${localLlmModel}` : ' Auto-detect'} ·
                            {useRawPages ? ' Raw pages' : ' Cropped panels'}
                          </div>
                        </div>
                        <span className={`text-[9px] font-bold px-1.5 py-0.5 rounded-md shrink-0 ${
                          ollamaOnline
                            ? 'bg-emerald-500/15 text-emerald-400'
                            : 'bg-amber-500/15 text-amber-400'
                        }`}>
                          {ollamaOnline ? 'ONLINE' : 'OFFLINE'}
                        </span>
                      </button>
                    </div>

                    {/* ── Collapsible Local AI Config ── */}
                    <button
                      onClick={() => setLocalLlmConfigOpen(!localLlmConfigOpen)}
                      className="flex items-center gap-1.5 text-[10px] text-zinc-500 hover:text-zinc-300 transition-colors"
                    >
                      <Settings2 size={10} />
                      {localLlmConfigOpen ? 'Hide' : 'Configure'} Local AI
                      <ChevronDown size={10} className={`transition-transform ${localLlmConfigOpen ? 'rotate-180' : ''}`} />
                    </button>

                    {localLlmConfigOpen && (
                      <div className="w-full rounded-xl p-4 space-y-3 animate-in fade-in slide-in-from-top-2 duration-200"
                        style={{ background: 'var(--surface-2)', border: '1px solid rgba(255,255,255,0.05)' }}>
                        {/* Engine selector */}
                        <div className="grid grid-cols-2 gap-1.5">
                          <button
                            onClick={() => {
                              setLocalLlmType('ollama')
                              if (localLlmUrl === 'http://localhost:1234/v1') {
                                setLocalLlmUrl('http://127.0.0.1:11434')
                              }
                            }}
                            className={`py-1.5 rounded-lg text-[10px] font-semibold transition-all ${
                              localLlmType === 'ollama'
                                ? 'bg-cyan-500/12 text-cyan-300 ring-1 ring-cyan-500/30'
                                : 'text-zinc-500 hover:text-zinc-300'
                            }`}
                            style={localLlmType !== 'ollama' ? { background: 'var(--surface-3)' } : undefined}
                          >
                            Ollama
                          </button>
                          <button
                            onClick={() => {
                              setLocalLlmType('lmstudio')
                              if (localLlmUrl === 'http://127.0.0.1:11434') {
                                setLocalLlmUrl('http://localhost:1234/v1')
                              }
                            }}
                            className={`py-1.5 rounded-lg text-[10px] font-semibold transition-all ${
                              localLlmType === 'lmstudio'
                                ? 'bg-cyan-500/12 text-cyan-300 ring-1 ring-cyan-500/30'
                                : 'text-zinc-500 hover:text-zinc-300'
                            }`}
                            style={localLlmType !== 'lmstudio' ? { background: 'var(--surface-3)' } : undefined}
                          >
                            LM Studio
                          </button>
                        </div>

                        {/* URL + Model — compact row */}
                        <div className="grid grid-cols-2 gap-2">
                          <div>
                            <label className="block text-[9px] font-bold text-zinc-500 uppercase mb-1">URL</label>
                            <input
                              type="text"
                              value={localLlmUrl}
                              onChange={(e) => setLocalLlmUrl(e.target.value)}
                              placeholder={localLlmType === 'ollama' ? 'localhost:11434' : 'localhost:1234/v1'}
                              className="w-full px-2 py-1.5 rounded-lg text-[10px] text-zinc-200 outline-none"
                              style={{ background: 'var(--surface-3)', border: '1px solid rgba(255,255,255,0.04)' }}
                            />
                          </div>
                          <div>
                            <label className="block text-[9px] font-bold text-zinc-500 uppercase mb-1">Model</label>
                            {localLlmType === 'ollama' ? (
                              <select
                                value={localLlmModel}
                                onChange={(e) => setLocalLlmModel(e.target.value)}
                                className="w-full px-2 py-1.5 rounded-lg text-[10px] text-zinc-200 outline-none"
                                style={{ background: 'var(--surface-3)', border: '1px solid rgba(255,255,255,0.04)' }}
                              >
                                <option value="">Auto-detect</option>
                                {visionModels.map((m) => (
                                  <option key={m.name} value={m.name}>{m.name}</option>
                                ))}
                              </select>
                            ) : (
                              <input
                                type="text"
                                value={localLlmModel}
                                onChange={(e) => setLocalLlmModel(e.target.value)}
                                placeholder="e.g. minicpm-v"
                                className="w-full px-2 py-1.5 rounded-lg text-[10px] text-zinc-200 outline-none"
                                style={{ background: 'var(--surface-3)', border: '1px solid rgba(255,255,255,0.04)' }}
                              />
                            )}
                          </div>
                        </div>

                        {/* Page mode — inline toggle */}
                        <div className="flex items-center gap-2">
                          <span className="text-[9px] font-bold text-zinc-500 uppercase">Pages:</span>
                          <button
                            onClick={() => setUseRawPages(true)}
                            className={`px-2.5 py-1 rounded-md text-[10px] font-medium transition-all ${
                              useRawPages ? 'bg-emerald-500/12 text-emerald-300 ring-1 ring-emerald-500/25' : 'text-zinc-500 hover:text-zinc-300'
                            }`}
                            style={!useRawPages ? { background: 'var(--surface-3)' } : undefined}
                          >
                            Raw
                          </button>
                          <button
                            onClick={() => setUseRawPages(false)}
                            className={`px-2.5 py-1 rounded-md text-[10px] font-medium transition-all ${
                              !useRawPages ? 'bg-emerald-500/12 text-emerald-300 ring-1 ring-emerald-500/25' : 'text-zinc-500 hover:text-zinc-300'
                            }`}
                            style={useRawPages ? { background: 'var(--surface-3)' } : undefined}
                          >
                            Crop/Split first
                          </button>
                        </div>
                      </div>
                    )}

                    {!geminiConfigured && !g0iConfigured && (
                      <p className="text-[10px] text-zinc-500 text-center max-w-sm leading-relaxed mt-1">
                        <strong className="text-zinc-300">No cloud key set.</strong>{' '}
                        Get a free Gemini key at{' '}
                        <a
                          href="#"
                          onClick={(e) => { e.preventDefault(); (window as any).electron?.openExternal?.('https://aistudio.google.com/apikey') }}
                          className="underline text-cyan-400 hover:text-cyan-300"
                        >
                          aistudio.google.com/apikey
                        </a>
                        {' '}— 30 seconds, no credit card.
                      </p>
                    )}
                  </div>
                </div>
              )}

              {narrating && (
                <div className="rounded-xl p-4 flex items-start gap-3" style={{ background: 'rgba(6,182,212,0.08)', border: '1px solid rgba(6,182,212,0.25)' }}>
                  <Loader2 size={16} className="text-cyan-400 animate-spin shrink-0 mt-0.5" />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center justify-between mb-1.5">
                      <span className="text-xs font-semibold text-cyan-300">Narrating</span>
                      <span className="text-[11px] text-zinc-500 tabular-nums">
                        {narrateProgress.total > 0 ? `${narrateProgress.current}/${narrateProgress.total}` : '…'}
                        {narrateProgress.estimatedRemainingSeconds != null && narrateProgress.estimatedRemainingSeconds > 0 && (
                          <span className="ml-1.5 text-zinc-400">
                            (Remaining: {formatEta(narrateProgress.estimatedRemainingSeconds * 1000)})
                          </span>
                        )}
                      </span>
                    </div>
                    <div className="h-1 rounded-full overflow-hidden" style={{ background: 'var(--surface-3)' }}>
                      <div
                        className="h-full rounded-full transition-all duration-300"
                        style={{ width: `${narrateProgress.percent}%`, background: 'linear-gradient(135deg, #06B6D4 0%, #6366F1 100%)' }}
                      />
                    </div>
                    {narrateProgress.label && (
                      <p className="text-[10px] text-zinc-500 mt-1.5 truncate">{narrateProgress.label}</p>
                    )}
                  </div>
                  <button
                    onClick={handleStopNarrate}
                    className="flex items-center gap-1 px-2.5 py-1.5 rounded-lg text-[11px] font-medium text-emerald-300 hover:text-emerald-200 shrink-0"
                    style={{ background: 'rgba(239,68,68,0.08)', border: '1px solid rgba(239,68,68,0.18)' }}
                    title="Stop narration (progress is saved — Resume continues)"
                  >
                    <Square size={11} /> Stop
                  </button>
                </div>
              )}

              {(narrations.gemini || narrations.g0i || narrations.ollama
                || narrationPartials.gemini || narrationPartials.g0i || narrationPartials.ollama) && (
                <div className="flex items-center gap-1.5 text-[11px]">
                  {(['gemini', 'g0i', 'ollama'] as const).map((prov) => {
                    const exists = !!narrations[prov] || !!narrationPartials[prov]?.batches?.length || !!narrationPartials[prov]?.descriptions?.length
                    const isActive = activeNarrationProvider === prov
                    return (
                      <button
                        key={prov}
                        disabled={!exists}
                        onClick={() => { setActiveNarrationProvider(prov); setNarrationHinglish(null) }}
                        className="px-2.5 py-1 rounded-md font-semibold transition-all disabled:opacity-30 disabled:cursor-not-allowed"
                        style={{
                          background: isActive
                            ? (prov === 'gemini'
                                ? 'linear-gradient(135deg, #06B6D4 0%, #6366F1 100%)'
                                : prov === 'g0i'
                                  ? 'linear-gradient(135deg, #60A5FA 0%, #2563EB 100%)'
                                : 'linear-gradient(135deg, #6366F1 0%, #059669 100%)')
                            : 'var(--surface-3)',
                          color: isActive ? '#fff' : 'var(--text-2)',
                        }}
                      >
                        {providerLabel(prov)}
                        {exists && (
                          <span className="ml-1 opacity-60">
                            ·{narrations[prov]?.batches.length ?? narrationPartials[prov]?.batches?.length ?? 0}b
                          </span>
                        )}
                      </button>
                    )
                  })}
                </div>
              )}

              {narration && (
                <>
                  <div className="flex items-center justify-between gap-2 text-[11px] text-zinc-500 flex-wrap">
                    <span>{narration.pages} pages · {narration.batches.length} batches · {narration.model}</span>
                    <div className="flex items-center gap-1.5">
                      <button
                        onClick={handleNarrationToAudio}
                        disabled={sendingToTts || !narration.narration}
                        className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[11px] font-semibold text-white transition-all hover:brightness-110 disabled:opacity-50"
                        style={{ background: 'linear-gradient(135deg, #059669 0%, #10B981 100%)' }}
                        title={`Save this ${providerLabel(activeNarrationProvider)} narration as page-aligned script, then generate synced audio`}
                      >
                        {sendingToTts ? <Loader2 size={11} className="animate-spin" /> : <AudioWaveform size={11} />}
                        Sync to Audio
                      </button>
                      <button
                        onClick={handleTranslateHinglish}
                        disabled={translatingHinglish || !narration.narration}
                        className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[11px] font-semibold text-white transition-all hover:brightness-110 disabled:opacity-50"
                        style={{ background: narrationHinglish ? 'var(--surface-3)' : 'linear-gradient(135deg, #F59E0B 0%, #10B981 100%)' }}
                        title="Translate the narration to Hinglish (Hindi in Roman script) using your local Ollama model"
                      >
                        {translatingHinglish ? <Loader2 size={11} className="animate-spin" /> : <Languages size={11} />}
                        {narrationHinglish ? 'Re-translate' : 'Hinglish'}
                      </button>
                      <button
                        onClick={() => {
                          navigator.clipboard.writeText(narrationHinglish ?? narration.narration)
                          toast.success(narrationHinglish ? 'Hinglish copied' : 'Narration copied')
                        }}
                        className="px-2.5 py-1 rounded-md text-[11px] text-zinc-300 hover:text-white transition-colors"
                        style={{ background: 'var(--surface-3)' }}
                      >
                        Copy
                      </button>
                      <button
                        onClick={async () => {
                          if (!selectedManga || !selectedChapter) return
                          const prov = activeNarrationProvider
                          if (!confirm(`Reset the ${providerLabel(prov)} narration for this chapter? This deletes it.`)) return
                          await invoke(IPC.TEXT_NARRATION_DISCARD, selectedManga.id, selectedChapter.id, prov).catch(() => {})
                          setNarrations((prev) => ({ ...prev, [prov]: undefined }))
                          setNarrationPartials((prev) => ({ ...prev, [prov]: undefined }))
                          setNarrationHinglish(null)
                          toast.success(`${providerLabel(prov)} narration reset`)
                        }}
                        className="flex items-center gap-1 px-2.5 py-1 rounded-md text-[11px] font-medium text-white transition-all hover:brightness-110"
                        style={{ background: 'rgba(239,68,68,0.85)' }}
                        title={`Delete the ${activeNarrationProvider} narration for this chapter`}
                      >
                        <Trash2 size={11} /> Reset
                      </button>
                    </div>
                  </div>
                  <div className="flex-1 overflow-y-auto rounded-xl p-5 text-[14px] leading-relaxed text-zinc-200 whitespace-pre-wrap font-serif"
                    style={{ background: 'var(--surface-2)', border: '1px solid rgba(255,255,255,0.04)' }}>
                    {narration.narration || (
                      <span className="text-zinc-600 italic">Narration is empty — model may have failed silently. Try again.</span>
                    )}
                  </div>
                  <div className="shrink-0 max-h-[46%] overflow-y-auto">
                    <div className="flex items-center justify-between mb-2 text-[11px]">
                      <span className="font-semibold" style={{ color: 'var(--text-2)' }}>Page-aligned panels</span>
                      <span style={{ color: 'var(--text-3)' }}>
                        {narration.page_from && narration.page_to
                          ? `Pages ${narration.page_from}-${narration.page_to} · ${narration.language || 'english'}`
                          : `${pageFiles.length || narration.pages} pages`}
                      </span>
                    </div>
                    <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                      {pageAlignedSegmentsFromNarration(narration, pageFiles.length || narration.pages).map((item) => {
                        const file = fileForPage(item.page)
                        const extracted = segments.find((segment) => segment.page === item.page)?.text
                        return (
                          <div key={item.page} className="rounded-lg overflow-hidden mv-card-2">
                            <div className="px-3 py-2 flex items-center justify-between border-b" style={{ borderColor: 'var(--panel-border)' }}>
                              <span className="text-[10px] font-semibold" style={{ color: 'var(--text-2)' }}>Page {item.page + 1}</span>
                              <span className="text-[9px] font-semibold" style={{ color: 'var(--accent-hover)' }}>
                                {providerLabel(activeNarrationProvider)}
                              </span>
                            </div>
                             {file && (
                              <div className="relative w-full max-h-64 overflow-hidden flex items-center justify-center bg-black/25">
                                <img
                                  src={pageUrl(file)}
                                  alt={`Page ${item.page + 1}`}
                                  className="max-h-64 object-contain"
                                />
                                {(() => {
                                  const fps = narration?.focus_points?.[String(item.page + 1)] || narrationPartial?.focus_points?.[String(item.page + 1)]
                                  return fps?.map((box, bIdx) => {
                                    const [ymin, xmin, ymax, xmax] = box
                                    return (
                                      <div
                                        key={bIdx}
                                        className="absolute border border-cyan-400 bg-cyan-400/10 pointer-events-none rounded"
                                        style={{
                                          top: `${ymin / 10}%`,
                                          left: `${xmin / 10}%`,
                                          width: `${(xmax - xmin) / 10}%`,
                                          height: `${(ymax - ymin) / 10}%`,
                                          boxShadow: '0 0 4px rgba(34,211,238,0.4)',
                                          zIndex: 5,
                                        }}
                                      />
                                    )
                                  })
                                })()}
                              </div>
                            )}
                            <div className="p-3 space-y-2">
                              {extracted && (
                                <div>
                                  <div className="text-[9px] uppercase mb-1" style={{ color: 'var(--text-3)' }}>Extracted text</div>
                                  <p className="text-[10px] leading-relaxed line-clamp-4" style={{ color: 'var(--text-3)' }}>{extracted}</p>
                                </div>
                              )}
                              <div>
                                <div className="text-[9px] uppercase mb-1" style={{ color: 'var(--accent-hover)' }}>Narration</div>
                                <p className="text-xs leading-relaxed whitespace-pre-wrap" style={{ color: 'var(--text-2)' }}>{item.text}</p>
                              </div>
                            </div>
                          </div>
                        )
                      })}
                    </div>
                  </div>
                  {narrationHinglish && (
                    <div className="rounded-xl p-5 text-[14px] leading-relaxed text-zinc-200 whitespace-pre-wrap font-serif"
                      style={{ background: 'rgba(245,158,11,0.05)', border: '1px solid rgba(245,158,11,0.2)', maxHeight: '40%', overflowY: 'auto' }}>
                      <div className="text-[10px] uppercase tracking-wider text-amber-400/80 mb-2 font-sans">Hinglish</div>
                      {narrationHinglish}
                    </div>
                  )}
                </>
              )}
            </div>
          )}
