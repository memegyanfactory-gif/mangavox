interface CommandTextareaProps {
  value: string
  onChange: (value: string) => void
  onKeyDown?: (event: React.KeyboardEvent<HTMLTextAreaElement>) => void
  placeholder?: string
  textareaRef?: React.RefObject<HTMLTextAreaElement | null>
  rows?: number
  className?: string
  style?: React.CSSProperties
}

/** Stable controlled textarea. Command highlighting lives outside the input so
 * the painted text and native caret can never drift apart. */
export function CommandTextarea({
  value, onChange, onKeyDown, placeholder, textareaRef, rows = 1, className = '', style,
}: CommandTextareaProps) {
  return (
    <div className="relative w-full">
      <textarea
        ref={textareaRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        rows={rows}
        placeholder={placeholder}
        className={`relative z-[1] w-full min-h-12 max-h-44 resize-none bg-transparent px-3 pt-3 pb-2 text-[14px] leading-6 outline-none ${className}`}
        style={{
          ...style,
          color: 'var(--text-1)',
          caretColor: 'var(--text-1)',
        }}
      />
    </div>
  )
}
