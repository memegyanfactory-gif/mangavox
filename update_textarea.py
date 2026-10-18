import re
with open("src/renderer/pages/TextPage.tsx", "r") as f:
    content = f.read()

# Update the textarea in the narrate block
old_textarea = """                    <textarea
                      value={narration?.narration || ''}
                      onChange={(e) => {
                         // Temporary placeholder logic: The user edits would ideally update local state
                      }}
                      placeholder="Write or edit the page narration..."
                      className="w-full h-full bg-transparent resize-none outline-none text-[15px] leading-[1.6] text-zinc-200 placeholder-zinc-600 font-serif"
                    />"""

new_textarea = """                    <textarea
                      value={editorText}
                      onChange={handleEditorChange}
                      placeholder="Write or edit the page narration..."
                      className="w-full h-full bg-transparent resize-none outline-none text-[15px] leading-[1.6] text-zinc-200 placeholder-zinc-600 font-serif"
                    />"""

content = content.replace(old_textarea, new_textarea)

old_status = """<CheckCircle2 size={12} className="text-emerald-500" />
                  Saved just now"""

new_status = """{editorAutosaveStatus === 'saving' ? (
                    <Loader2 size={12} className="text-zinc-500 animate-spin" />
                  ) : editorAutosaveStatus === 'saved' ? (
                    <CheckCircle2 size={12} className="text-emerald-500" />
                  ) : null}
                  {editorAutosaveStatus === 'saving' ? 'Saving...' : 
                   editorAutosaveStatus === 'saved' && editorLastSaved ? `Saved at ${editorLastSaved.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : ''}"""

content = content.replace(old_status, new_status)

with open("src/renderer/pages/TextPage.tsx", "w") as f:
    f.write(content)

