import { useState } from 'react'
import { Button, Card, Input } from '@botho/ui'
import { MIN_PASSWORD_LENGTH, passwordStrength, type PasswordStrength } from '@botho/core'
import { AlertCircle, Lock, RefreshCw } from 'lucide-react'

const STRENGTH_META: Record<PasswordStrength, { label: string; bars: number; color: string }> = {
  'too-short': { label: `At least ${MIN_PASSWORD_LENGTH} characters`, bars: 0, color: 'bg-danger' },
  weak: { label: 'Weak password', bars: 1, color: 'bg-danger' },
  fair: { label: 'Fair password', bars: 2, color: 'bg-warning' },
  strong: { label: 'Strong password', bars: 3, color: 'bg-success' },
}

/**
 * Shared password + confirm fields with a simple strength hint (#475). Encrypts
 * the wallet by default; a password is required to proceed.
 */
export function PasswordFields({
  password,
  confirmPassword,
  onPassword,
  onConfirmPassword,
}: {
  password: string
  confirmPassword: string
  onPassword: (v: string) => void
  onConfirmPassword: (v: string) => void
}) {
  const passwordsMatch = password === confirmPassword
  const strength = passwordStrength(password)
  const meta = STRENGTH_META[strength]

  return (
    <div className="space-y-3">
      <div>
        <Input
          type="password"
          placeholder={`Password (min ${MIN_PASSWORD_LENGTH} characters)`}
          value={password}
          onChange={(e: React.ChangeEvent<HTMLInputElement>) => onPassword(e.target.value)}
        />
        {password.length > 0 && (
          <div className="mt-2">
            <div className="flex gap-1">
              {[0, 1, 2].map((i) => (
                <div
                  key={i}
                  className={`h-1 flex-1 rounded-full ${i < meta.bars ? meta.color : 'bg-steel'}`}
                />
              ))}
            </div>
            <p className="text-xs text-ghost mt-1">{meta.label}</p>
          </div>
        )}
      </div>
      <div>
        <Input
          type="password"
          placeholder="Confirm password"
          value={confirmPassword}
          onChange={(e: React.ChangeEvent<HTMLInputElement>) => onConfirmPassword(e.target.value)}
        />
        {confirmPassword && !passwordsMatch && (
          <p className="text-xs text-danger mt-1">Passwords don't match</p>
        )}
      </div>
    </div>
  )
}

/** True when a password is valid to encrypt a wallet (#475). */
export function isPasswordValid(password: string, confirmPassword: string): boolean {
  return password.length >= MIN_PASSWORD_LENGTH && password === confirmPassword
}

/**
 * Modal to SET a password on a plaintext wallet, or CHANGE an existing one
 * (#489). Reuses the #475 password policy (min length, strength hint, confirm
 * field) via {@link PasswordFields}; the change flow adds a current-password
 * field. On submit it calls the wallet context, which re-wraps the seed +
 * address book + claim links under the new key.
 *
 * Shared so both the wallet settings (#489) and the contacts page (#488) can
 * route a plaintext-wallet user into "Set a password" without duplicating the
 * modal.
 */
export function PasswordSettingsModal({
  mode,
  onClose,
  onSetPassword,
  onChangePassword,
}: {
  mode: 'set' | 'change'
  onClose: () => void
  onSetPassword: (newPassword: string) => Promise<void>
  onChangePassword: (oldPassword: string, newPassword: string) => Promise<void>
}) {
  const [oldPassword, setOldPassword] = useState('')
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [isSaving, setIsSaving] = useState(false)

  const isChange = mode === 'change'
  const canSubmit =
    isPasswordValid(password, confirmPassword) && (!isChange || oldPassword.length > 0)

  const handleSubmit = async () => {
    setError(null)
    setIsSaving(true)
    try {
      if (isChange) {
        await onChangePassword(oldPassword, password)
      } else {
        await onSetPassword(password)
      }
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not update password')
    } finally {
      setIsSaving(false)
    }
  }

  return (
    <div className="fixed inset-0 bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4 z-50">
      <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl">
        <div className="text-center mb-5 sm:mb-6">
          <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
            <Lock className="text-pulse" size={28} />
          </div>
          <h3 className="font-display text-lg sm:text-xl font-semibold mb-2">
            {isChange ? 'Change password' : 'Set a password'}
          </h3>
          <p className="text-ghost text-sm">
            {isChange
              ? 'Enter your current password, then choose a new one. Your old password will stop working.'
              : 'Encrypt your wallet on this device. Keep your recovery phrase safe — a forgotten password cannot be recovered.'}
          </p>
        </div>

        <div className="space-y-3">
          {isChange && (
            <Input
              type="password"
              placeholder="Current password"
              value={oldPassword}
              onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
                setOldPassword(e.target.value)
                setError(null)
              }}
              autoFocus
            />
          )}
          <PasswordFields
            password={password}
            confirmPassword={confirmPassword}
            onPassword={(v) => { setPassword(v); setError(null) }}
            onConfirmPassword={(v) => { setConfirmPassword(v); setError(null) }}
          />

          {error && (
            <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
              <AlertCircle size={16} className="shrink-0" />
              <span>{error}</span>
            </div>
          )}

          <div className="space-y-3 pt-1">
            <Button onClick={handleSubmit} disabled={!canSubmit || isSaving} className="w-full justify-center">
              {isSaving ? (
                <><RefreshCw size={16} className="mr-2 animate-spin" />Saving...</>
              ) : (
                isChange ? 'Change password' : 'Set password'
              )}
            </Button>
            <Button variant="secondary" onClick={onClose} disabled={isSaving} className="w-full justify-center">
              Cancel
            </Button>
          </div>
        </div>
      </Card>
    </div>
  )
}
