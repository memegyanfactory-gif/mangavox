import re
with open("src/renderer/pages/TextPage.tsx", "r") as f:
    content = f.read()

# find a good place to insert local state
idx = content.find("const [manualText, setManualText] = useState('')")
if idx == -1:
    print("Could not find insert point")
    exit(1)

state_vars = """
  // ── Text Panel Redesign State ──
  const [editorText, setEditorText] = useState('')
  const [editorAutosaveStatus, setEditorAutosaveStatus] = useState<'saved' | 'saving' | 'error'>('saved')
  const [editorLastSaved, setEditorLastSaved] = useState<Date | null>(null)
  
  // Sync the local editor text whenever the backend narration changes
  useEffect(() => {
    if (narration?.narration) {
      setEditorText(narration.narration)
    } else {
      setEditorText('')
    }
  }, [narration?.narration, activeNarrationProvider])

  const handleEditorChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const newVal = e.target.value
    setEditorText(newVal)
    setEditorAutosaveStatus('saving')
    
    // Autosave to the active provider's narration object
    // Note: To fully support saving back to the file system, we should call an IPC endpoint.
    // For now, we update the local `narrations` state.
    setNarrations((prev) => {
      const existing = prev[activeNarrationProvider]
      if (!existing) return prev
      return {
        ...prev,
        [activeNarrationProvider]: {
          ...existing,
          narration: newVal
        }
      }
    })
    
    // Fake a quick save status update
    setTimeout(() => {
      setEditorAutosaveStatus('saved')
      setEditorLastSaved(new Date())
    }, 500)
  }

"""

content = content[:idx] + state_vars + content[idx:]

with open("src/renderer/pages/TextPage.tsx", "w") as f:
    f.write(content)

