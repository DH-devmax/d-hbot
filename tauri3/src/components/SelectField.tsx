import { useEffect, useId, useRef, useState } from 'react'
import { Check, ChevronDown } from 'lucide-react'

export type SelectOption = {
  value: string
  label: string
  description?: string
}

export default function SelectField({
  label,
  value,
  options,
  onChange,
  disabled = false,
  className = '',
}: {
  label: string
  value: string
  options: SelectOption[]
  onChange: (value: string) => void
  disabled?: boolean
  className?: string
}) {
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const listId = useId()
  const selected = options.find(option => option.value === value)

  useEffect(() => {
    if (!open) return
    const closeOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false)
    }
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      setOpen(false)
      triggerRef.current?.focus()
    }
    document.addEventListener('pointerdown', closeOutside)
    document.addEventListener('keydown', closeOnEscape)
    return () => {
      document.removeEventListener('pointerdown', closeOutside)
      document.removeEventListener('keydown', closeOnEscape)
    }
  }, [open])

  return <div className={`select-field ${className}`.trim()} ref={rootRef}>
    <span className="select-field__label">{label}</span>
    <button
      ref={triggerRef}
      type="button"
      className="select-field__trigger"
      role="combobox"
      aria-label={label}
      aria-controls={listId}
      aria-expanded={open}
      aria-haspopup="listbox"
      disabled={disabled}
      onClick={() => setOpen(current => !current)}
    >
      <span>{selected?.label || '请选择'}</span>
      <ChevronDown size={15} aria-hidden="true" />
    </button>
    {open && <div className="select-field__menu" id={listId} role="listbox" aria-label={`${label}选项`}>
      {options.map(option => <button
        type="button"
        role="option"
        aria-selected={option.value === value}
        className={option.value === value ? 'selected' : ''}
        key={option.value}
        onClick={() => {
          onChange(option.value)
          setOpen(false)
          triggerRef.current?.focus()
        }}
      >
        <span><strong>{option.label}</strong>{option.description && <small>{option.description}</small>}</span>
        {option.value === value && <Check size={14} aria-hidden="true" />}
      </button>)}
    </div>}
  </div>
}
