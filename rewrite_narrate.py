import re
import os

with open("src/renderer/pages/TextPage.tsx", "r") as f:
    content = f.read()

# We want to replace the `activeTab === 'narrate'` block.
start_idx = content.find("{activeTab === 'narrate' && selectedChapter && (")
if start_idx == -1:
    print("Could not find start")
    exit(1)

# Find the end of the `activeTab === 'narrate'` block.
# We know it ends before `{activeTab === 'manual' && (`
end_idx = content.find("{activeTab === 'manual' && (", start_idx)
if end_idx == -1:
    print("Could not find end")
    exit(1)

new_block = """{activeTab === 'narrate' && selectedChapter && (
            <div className="flex flex-col h-full bg-[var(--surface-1)]">
              {/* Top Bar */}
              <div className="flex items-center justify-between px-6 py-3 border-b border-white/5 shrink-0">
                <div className="flex items-center gap-4">
                  <div className="flex items-center gap-2">
                    <span className="text-xs font-medium text-zinc-400">Content:</span>
                    <button className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[var(--surface-2)] hover:bg-[var(--surface-3)] text-xs text-zinc-200 transition-colors">
                      <FileText size={14} className="text-emerald-400" />
                      Narration
                      <ChevronDown size={12} className="text-zinc-500" />
                    </button>
                  </div>
                  
                  <div className="w-px h-4 bg-white/10" />
                  
                  <div className="flex items-center gap-2 relative">
                    <span className="text-xs font-medium text-zinc-400">AI Model:</span>
                    <button 
                      onClick={() => setOllamaModelMenuOpen(!ollamaModelMenuOpen)}
                      className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[var(--surface-2)] hover:bg-[var(--surface-3)] text-xs text-zinc-200 transition-colors"
                    >
                      <Sparkles size={14} className="text-cyan-400" />
                      {activeNarrationProvider === 'gemini' ? 'Gemini 2.5 Flash' : 
                       activeNarrationProvider === 'g0i' ? 'g0i.ai (Gemini)' : 
                       localLlmModel || 'Local Model'}
                      <ChevronDown size={12} className="text-zinc-500" />
                    </button>
                    {/* Unified Model Dropdown (simplified inline for now) */}
                    {ollamaModelMenuOpen && (
                      <div className="absolute top-full left-0 mt-2 w-64 rounded-xl bg-[var(--surface-2)] border border-white/10 shadow-xl z-50 p-2">
                         <div className="px-2 py-1.5 text-[10px] font-bold tracking-wider text-zinc-500 uppercase">Cloud Models</div>
                         <button onClick={() => { setActiveNarrationProvider('gemini'); setOllamaModelMenuOpen(false); }} className="w-full flex items-center justify-between px-2 py-1.5 rounded-lg hover:bg-[var(--surface-3)] text-xs text-zinc-300">
                           <span className="flex items-center gap-2"><Sparkles size={12} className="text-cyan-400"/> Gemini API</span>
                           {geminiConfigured ? <span className="text-[9px] text-emerald-400 bg-emerald-400/10 px-1.5 py-0.5 rounded">Ready</span> : <span className="text-[9px] text-amber-400 bg-amber-400/10 px-1.5 py-0.5 rounded">Missing Key</span>}
                         </button>
                         <button onClick={() => { setActiveNarrationProvider('g0i'); setOllamaModelMenuOpen(false); }} className="w-full flex items-center justify-between px-2 py-1.5 rounded-lg hover:bg-[var(--surface-3)] text-xs text-zinc-300">
                           <span className="flex items-center gap-2"><Zap size={12} className="text-blue-400"/> g0i.ai Gateway</span>
                           {g0iConfigured ? <span className="text-[9px] text-emerald-400 bg-emerald-400/10 px-1.5 py-0.5 rounded">Ready</span> : <span className="text-[9px] text-amber-400 bg-amber-400/10 px-1.5 py-0.5 rounded">Missing Key</span>}
                         </button>
                         <div className="px-2 py-1.5 text-[10px] font-bold tracking-wider text-zinc-500 uppercase mt-2">Local Models</div>
                         <button onClick={() => { setActiveNarrationProvider('ollama'); setLocalLlmType('ollama'); setOllamaModelMenuOpen(false); }} className="w-full flex items-center justify-between px-2 py-1.5 rounded-lg hover:bg-[var(--surface-3)] text-xs text-zinc-300">
                           <span className="flex items-center gap-2"><Cpu size={12} className="text-emerald-400"/> Ollama Vision</span>
                           {ollamaOnline ? <span className="text-[9px] text-emerald-400 bg-emerald-400/10 px-1.5 py-0.5 rounded">Online</span> : <span className="text-[9px] text-zinc-500 bg-white/5 px-1.5 py-0.5 rounded">Offline</span>}
                         </button>
                         <button onClick={() => { setActiveNarrationProvider('ollama'); setLocalLlmType('lmstudio'); setOllamaModelMenuOpen(false); }} className="w-full flex items-center justify-between px-2 py-1.5 rounded-lg hover:bg-[var(--surface-3)] text-xs text-zinc-300">
                           <span className="flex items-center gap-2"><Cpu size={12} className="text-emerald-400"/> LM Studio</span>
                           <span className="text-[9px] text-emerald-400 bg-emerald-400/10 px-1.5 py-0.5 rounded">Ready</span>
                         </button>
                      </div>
                    )}
                  </div>
                </div>

                <div className="flex items-center gap-4">
                  <div className="text-[11px] text-zinc-500 font-medium">
                    {narration?.narration?.trim().split(/\s+/).filter(Boolean).length || 0} words
                  </div>
                  <div className="flex items-center gap-1">
                    <button className="p-1.5 rounded hover:bg-[var(--surface-3)] text-zinc-400 transition-colors" title="Undo"><RotateCcw size={14} /></button>
                    {/* Add Redo icon if available, skipped for brevity */}
                  </div>
                  <button className="p-1.5 rounded hover:bg-[var(--surface-3)] text-zinc-400 transition-colors" title="More options">
                    <Settings2 size={14} />
                  </button>
                </div>
              </div>

              {/* Text Area */}
              <div className="flex-1 overflow-y-auto relative custom-scrollbar">
                <div className="max-w-3xl mx-auto h-full px-8 py-8">
                  {narrating ? (
                    <div className="flex flex-col items-center justify-center h-full text-zinc-500 space-y-4">
                      <Loader2 size={32} className="text-cyan-400 animate-spin" />
                      <div className="text-sm font-medium text-cyan-300">Generating Narration...</div>
                      <div className="text-xs text-zinc-500">{narrateProgress.current}/{narrateProgress.total} pages processed</div>
                    </div>
                  ) : (
                    <textarea
                      value={narration?.narration || ''}
                      onChange={(e) => {
                         // Temporary placeholder logic: The user edits would ideally update local state
                      }}
                      placeholder="Write or edit the page narration..."
                      className="w-full h-full bg-transparent resize-none outline-none text-[15px] leading-[1.6] text-zinc-200 placeholder-zinc-600 font-serif"
                    />
                  )}
                </div>
              </div>

              {/* Bottom Action Bar */}
              <div className="flex items-center justify-between px-6 py-4 border-t border-white/5 shrink-0 bg-[var(--surface-1)]">
                <div className="flex items-center gap-2 text-[11px] text-zinc-500">
                  <CheckCircle2 size={12} className="text-emerald-500" />
                  Saved just now
                </div>
                
                <div className="flex items-center gap-3">
                  <button 
                    onClick={() => handleNarrate(activeNarrationProvider)}
                    className="px-4 py-2 rounded-xl text-xs font-medium text-zinc-300 hover:text-white bg-[var(--surface-2)] hover:bg-[var(--surface-3)] transition-colors"
                  >
                    Regenerate
                  </button>
                  <button 
                    onClick={() => {
                      if (narration?.narration) {
                        navigator.clipboard.writeText(narration.narration);
                        toast.success("Copied to clipboard");
                      }
                    }}
                    className="px-4 py-2 rounded-xl text-xs font-medium text-zinc-300 hover:text-white bg-[var(--surface-2)] hover:bg-[var(--surface-3)] transition-colors"
                  >
                    Copy
                  </button>
                  <button 
                    onClick={handleNarrationToAudio}
                    className="px-5 py-2 rounded-xl text-xs font-semibold text-teal-950 bg-teal-400 hover:bg-teal-300 transition-colors shadow-[0_0_15px_rgba(45,212,191,0.2)]"
                  >
                    Mark as reviewed
                  </button>
                </div>
              </div>
            </div>
          )}
"""

content = content[:start_idx] + new_block + content[end_idx:]

with open("src/renderer/pages/TextPage.tsx", "w") as f:
    f.write(content)

