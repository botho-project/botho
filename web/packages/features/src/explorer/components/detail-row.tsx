export interface DetailRowProps {
  label: string
  value: string
  valueClass?: string
  mono?: boolean
  onClick?: () => void
}

export function DetailRow({ label, value, valueClass, mono, onClick }: DetailRowProps) {
  return (
    <div>
      <p className="text-xs uppercase tracking-wider text-[--color-dim]">{label}</p>
      <p
        className={`mt-1 text-sm ${mono ? 'font-mono' : ''} ${valueClass || 'text-[--color-light]'} ${
          onClick ? 'cursor-pointer hover:text-[--color-pulse] hover:underline' : ''
        }`}
        onClick={onClick}
      >
        {value}
      </p>
    </div>
  )
}
