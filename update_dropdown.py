import re
with open("src/renderer/pages/TextPage.tsx", "r") as f:
    content = f.read()

start_str = "{/* Unified Model Dropdown (simplified inline for now) */}"
end_str = "                    )}\n                  </div>"

start_idx = content.find(start_str)
end_idx = content.find(end_str, start_idx)

if start_idx != -1 and end_idx != -1:
    new_menu = """{/* Unified Model Dropdown */}
                    {ollamaModelMenuOpen && (
                      <div className="absolute top-full left-0 mt-2 w-72 rounded-xl bg-[var(--surface-2)] border border-white/10 shadow-xl z-50 p-2 max-h-96 overflow-y-auto custom-scrollbar">
                         
                         {/* Cloud Models */}
                         <div className="px-2 py-1.5 text-[10px] font-bold tracking-wider text-zinc-500 uppercase mt-1">Cloud Models</div>
                         <button onClick={() => { setActiveNarrationProvider('gemini'); setOllamaModelMenuOpen(false); }} className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-zinc-300 ${activeNarrationProvider === 'gemini' ? 'bg-[var(--surface-3)]' : ''}`}>
                           <span className="flex items-center gap-2"><Sparkles size={12} className="text-cyan-400"/> Gemini API</span>
                           {geminiConfigured ? <span className="text-[9px] text-emerald-400 bg-emerald-400/10 px-1.5 py-0.5 rounded font-medium">Ready</span> : <span className="text-[9px] text-amber-400 bg-amber-400/10 px-1.5 py-0.5 rounded font-medium">Missing Key</span>}
                         </button>
                         <button onClick={() => { setActiveNarrationProvider('g0i'); setOllamaModelMenuOpen(false); }} className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-zinc-300 ${activeNarrationProvider === 'g0i' ? 'bg-[var(--surface-3)]' : ''}`}>
                           <span className="flex items-center gap-2"><Zap size={12} className="text-blue-400"/> g0i.ai Gateway</span>
                           {g0iConfigured ? <span className="text-[9px] text-emerald-400 bg-emerald-400/10 px-1.5 py-0.5 rounded font-medium">Ready</span> : <span className="text-[9px] text-amber-400 bg-amber-400/10 px-1.5 py-0.5 rounded font-medium">Missing Key</span>}
                         </button>
                         
                         {/* Local Models */}
                         <div className="px-2 py-1.5 text-[10px] font-bold tracking-wider text-zinc-500 uppercase mt-3">Local Models (Ollama)</div>
                         {visionModels.length > 0 ? visionModels.map(m => (
                           <button key={m.name} onClick={() => { setActiveNarrationProvider('ollama'); setLocalLlmType('ollama'); setNarrateOllamaModel(m.name); setOllamaModelMenuOpen(false); }} className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-zinc-300 ${activeNarrationProvider === 'ollama' && localLlmType === 'ollama' && narrateOllamaModel === m.name ? 'bg-[var(--surface-3)]' : ''}`}>
                             <span className="flex items-center gap-2 truncate pr-2"><Cpu size={12} className="text-emerald-400 shrink-0"/> <span className="truncate">{m.name}</span></span>
                             {ollamaOnline ? <span className="text-[9px] text-emerald-400 bg-emerald-400/10 px-1.5 py-0.5 rounded shrink-0 font-medium">Online</span> : <span className="text-[9px] text-zinc-500 bg-white/5 px-1.5 py-0.5 rounded shrink-0 font-medium">Offline</span>}
                           </button>
                         )) : (
                           <div className="px-2 py-1.5 text-xs text-zinc-500 italic">No Ollama vision models found</div>
                         )}

                         <div className="px-2 py-1.5 text-[10px] font-bold tracking-wider text-zinc-500 uppercase mt-3">Local Models (LM Studio)</div>
                         <button onClick={() => { setActiveNarrationProvider('ollama'); setLocalLlmType('lmstudio'); setOllamaModelMenuOpen(false); }} className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-zinc-300 ${activeNarrationProvider === 'ollama' && localLlmType === 'lmstudio' ? 'bg-[var(--surface-3)]' : ''}`}>
                           <span className="flex items-center gap-2"><Cpu size={12} className="text-emerald-400"/> LM Studio (Auto)</span>
                           <span className="text-[9px] text-emerald-400 bg-emerald-400/10 px-1.5 py-0.5 rounded font-medium">Ready</span>
                         </button>

                      </div>
"""
    
    content = content[:start_idx] + new_menu + content[end_idx:]
    with open("src/renderer/pages/TextPage.tsx", "w") as f:
        f.write(content)
else:
    print("Failed to replace menu")

