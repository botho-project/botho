import { useMemo, useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, Input, Logo } from '@botho/ui'
import { isValidAddress } from '@botho/core'
import type { Contact } from '@botho/core'
import {
  ArrowLeft,
  Users,
  Search,
  Plus,
  Pencil,
  Trash2,
  X,
  Check,
  AlertCircle,
  Tag,
  Lock,
} from 'lucide-react'
import { useWallet } from '../contexts/wallet'
import { PasswordSettingsModal } from '../components/PasswordSettingsModal'

type SortBy = 'name' | 'lastTx' | 'txCount'

const SORT_LABELS: Record<SortBy, string> = {
  name: 'Name',
  lastTx: 'Recent',
  txCount: 'Most paid',
}

/** Truncate an address for compact display. */
function shortAddress(address: string, len = 10): string {
  if (address.length <= len * 2 + 3) return address
  return `${address.slice(0, len)}…${address.slice(-len)}`
}

/**
 * Sort + filter the contact list for display. Mirrors `AddressBook.getAll`
 * sorting and `AddressBook.search` filtering, but works on the reactive
 * `contacts` array from the wallet context so the list updates live.
 */
function arrange(contacts: Contact[], sortBy: SortBy, query: string): Contact[] {
  const q = query.trim().toLowerCase()
  const filtered = q
    ? contacts.filter(
        (c) =>
          c.name.toLowerCase().includes(q) ||
          c.address.toLowerCase().includes(q),
      )
    : contacts.slice()

  switch (sortBy) {
    case 'name':
      // Unnamed (blank) entries sort last so labelled contacts lead.
      return filtered.sort((a, b) => {
        const an = a.name.trim()
        const bn = b.name.trim()
        if (!an && bn) return 1
        if (an && !bn) return -1
        return an.localeCompare(bn)
      })
    case 'lastTx':
      return filtered.sort((a, b) => (b.lastTxAt ?? 0) - (a.lastTxAt ?? 0))
    case 'txCount':
      return filtered.sort((a, b) => b.txCount - a.txCount)
    default:
      return filtered
  }
}

/**
 * Contacts manager (#472). Lists saved/previously-paid addresses, lets the user
 * sort (name / recent / most-paid), search (name + address), and add / edit /
 * delete with an editable name + notes annotation. Unnamed "previously paid"
 * entries (auto-created on send) show as "Unnamed — tap to label".
 */
export function ContactsPage() {
  const {
    contacts,
    addContact,
    updateContact,
    deleteContact,
    hasWallet,
    isEncrypted,
    isLocked,
    setPassword,
    changePassword,
  } = useWallet()

  const [sortBy, setSortBy] = useState<SortBy>('name')
  const [query, setQuery] = useState('')
  const [editing, setEditing] = useState<Contact | null>(null)
  const [adding, setAdding] = useState(false)
  const [showPasswordModal, setShowPasswordModal] = useState(false)

  // Contacts are encrypted at rest under the wallet's vault key (#476). When
  // there is no vault key, the encrypted address book's save() is a deliberate
  // silent NO-OP — it must never write the contact graph in cleartext. There is
  // no key in two cases:
  //   - PLAINTEXT wallet (a legacy no-password wallet): isEncrypted === false.
  //   - LOCKED wallet (encrypted but not unlocked this session): isLocked.
  // In either case adding/editing a contact would silently fail to persist
  // ("silent broken button"), so we surface a hint and gate the controls
  // instead of letting the user type into a field that discards their input
  // (#488). Encrypted + unlocked wallets are unaffected.
  const plaintextWallet = hasWallet && !isEncrypted
  const lockedWallet = isLocked
  const canPersistContacts = !plaintextWallet && !lockedWallet

  const arranged = useMemo(
    () => arrange(contacts, sortBy, query),
    [contacts, sortBy, query],
  )

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/wallet" className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">
              Botho Wallet
            </span>
          </Link>
        </div>
      </header>

      <main className="py-6 sm:py-8 md:py-12">
        <div className="max-w-2xl mx-auto px-4 sm:px-0 space-y-4 sm:space-y-6">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Users className="text-pulse" size={22} />
              <h1 className="font-display text-xl sm:text-2xl font-bold">Contacts</h1>
            </div>
            <Button
              onClick={() => setAdding(true)}
              disabled={!canPersistContacts}
              title={
                canPersistContacts
                  ? undefined
                  : plaintextWallet
                    ? 'Add a password to your wallet to save contacts'
                    : 'Unlock your wallet to save contacts'
              }
            >
              <Plus size={16} className="mr-2" />Add
            </Button>
          </div>

          {/* Persistence hint (#488): contacts are encrypted at rest and cannot
              be saved without a vault key — surface why instead of silently
              discarding the user's input. */}
          {plaintextWallet && (
            <Card className="p-4 border border-pulse/30 bg-pulse/5">
              <div className="flex items-start gap-3">
                <Lock size={18} className="text-pulse shrink-0 mt-0.5" />
                <div className="space-y-2">
                  <p className="text-sm text-light">
                    Contacts require a password-protected wallet
                  </p>
                  <p className="text-xs text-ghost">
                    Your wallet has no password, so contacts can&apos;t be saved —
                    they&apos;re encrypted on this device and never stored in
                    cleartext. Add a password to start saving contacts.
                  </p>
                  <Button size="sm" onClick={() => setShowPasswordModal(true)}>
                    Set a password
                  </Button>
                </div>
              </div>
            </Card>
          )}

          {lockedWallet && (
            <Card className="p-4 border border-pulse/30 bg-pulse/5">
              <div className="flex items-start gap-3">
                <Lock size={18} className="text-pulse shrink-0 mt-0.5" />
                <div className="space-y-2">
                  <p className="text-sm text-light">Wallet is locked</p>
                  <p className="text-xs text-ghost">
                    Unlock your wallet to view and save contacts. They&apos;re
                    encrypted on this device and unavailable while locked.
                  </p>
                  <Link to="/wallet">
                    <Button size="sm">Go to wallet to unlock</Button>
                  </Link>
                </div>
              </div>
            </Card>
          )}

          {/* Search */}
          <div className="relative">
            <Search
              size={16}
              className="absolute left-3 top-1/2 -translate-y-1/2 text-ghost"
            />
            <Input
              type="text"
              placeholder="Search by name or address…"
              value={query}
              onChange={(e: React.ChangeEvent<HTMLInputElement>) =>
                setQuery(e.target.value)
              }
              className="pl-9"
            />
          </div>

          {/* Sort toggle */}
          <div className="flex rounded-lg bg-abyss border border-steel p-1">
            {(Object.keys(SORT_LABELS) as SortBy[]).map((key) => (
              <button
                key={key}
                onClick={() => setSortBy(key)}
                className={`flex-1 py-2 px-3 rounded-md text-xs sm:text-sm font-medium transition-colors ${
                  sortBy === key
                    ? 'bg-steel text-light'
                    : 'text-ghost hover:text-light'
                }`}
              >
                {SORT_LABELS[key]}
              </button>
            ))}
          </div>

          {/* List */}
          {arranged.length === 0 ? (
            <Card className="p-6 text-center text-ghost text-sm">
              {contacts.length === 0
                ? 'No contacts yet. Addresses you pay will appear here, ready to label.'
                : 'No contacts match your search.'}
            </Card>
          ) : (
            <div className="space-y-2">
              {arranged.map((c) => (
                <ContactRow
                  key={c.id}
                  contact={c}
                  editable={canPersistContacts}
                  onEdit={() => canPersistContacts && setEditing(c)}
                />
              ))}
            </div>
          )}
        </div>
      </main>

      {adding && canPersistContacts && (
        <ContactEditor
          mode="add"
          onClose={() => setAdding(false)}
          onSave={async ({ name, address, notes }) => {
            await addContact(name, address, notes)
          }}
        />
      )}

      {editing && canPersistContacts && (
        <ContactEditor
          mode="edit"
          contact={editing}
          onClose={() => setEditing(null)}
          onSave={async ({ name, address, notes }) => {
            await updateContact(editing.id, { name, address, notes })
          }}
          onDelete={async () => {
            await deleteContact(editing.id)
          }}
        />
      )}

      {/* Reuse the shared #489 set-password flow so a plaintext-wallet user can
          upgrade to an encrypted wallet and start saving contacts (#488). */}
      {showPasswordModal && (
        <PasswordSettingsModal
          mode="set"
          onClose={() => setShowPasswordModal(false)}
          onSetPassword={setPassword}
          onChangePassword={changePassword}
        />
      )}
    </div>
  )
}

function ContactRow({
  contact,
  editable = true,
  onEdit,
}: {
  contact: Contact
  editable?: boolean
  onEdit: () => void
}) {
  const named = contact.name.trim().length > 0
  return (
    <Card className="p-3 sm:p-4 flex items-center justify-between gap-3">
      <button
        onClick={onEdit}
        disabled={!editable}
        className="flex-1 min-w-0 text-left disabled:cursor-default"
        title={
          editable
            ? named
              ? 'Edit contact'
              : 'Tap to label this address'
            : undefined
        }
      >
        <div className="flex items-center gap-2">
          <span
            className={`font-medium truncate ${
              named ? 'text-light' : 'text-ghost italic'
            }`}
          >
            {named ? contact.name : 'Unnamed — tap to label'}
          </span>
          {contact.txCount > 0 && (
            <span className="shrink-0 text-[11px] text-ghost bg-abyss border border-steel rounded-full px-2 py-0.5">
              {contact.txCount}× paid
            </span>
          )}
        </div>
        <p className="font-mono text-xs text-ghost mt-0.5 truncate">
          {shortAddress(contact.address)}
        </p>
        {contact.notes && (
          <p className="text-xs text-ghost/80 mt-1 truncate">{contact.notes}</p>
        )}
      </button>
      <Button variant="ghost" size="sm" onClick={onEdit} disabled={!editable} title="Edit">
        {named ? <Pencil size={16} /> : <Tag size={16} />}
      </Button>
    </Card>
  )
}

function ContactEditor({
  mode,
  contact,
  onClose,
  onSave,
  onDelete,
}: {
  mode: 'add' | 'edit'
  contact?: Contact
  onClose: () => void
  onSave: (data: { name: string; address: string; notes?: string }) => Promise<void>
  onDelete?: () => Promise<void>
}) {
  const [name, setName] = useState(contact?.name ?? '')
  const [address, setAddress] = useState(contact?.address ?? '')
  const [notes, setNotes] = useState(contact?.notes ?? '')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState(false)

  // In edit mode the address is fixed (it's the identity of the entry); editing
  // it could collide with another contact. Allow editing only when adding.
  const addressEditable = mode === 'add'

  const handleSave = async () => {
    setError(null)
    if (!address.trim()) {
      setError('Enter an address.')
      return
    }
    if (!isValidAddress(address.trim())) {
      setError('That does not look like a valid Botho address.')
      return
    }
    setBusy(true)
    try {
      await onSave({
        name: name.trim(),
        address: address.trim(),
        notes: notes.trim() || undefined,
      })
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not save contact.')
    } finally {
      setBusy(false)
    }
  }

  const handleDelete = async () => {
    if (!onDelete) return
    setBusy(true)
    try {
      await onDelete()
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not delete contact.')
      setBusy(false)
    }
  }

  return (
    <div className="fixed inset-0 bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4 z-50">
      <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl max-h-[92vh] overflow-y-auto">
        <div className="flex items-center justify-between mb-5">
          <h3 className="font-display text-lg font-semibold">
            {mode === 'add' ? 'Add contact' : 'Edit contact'}
          </h3>
          <button onClick={onClose} className="text-ghost hover:text-light" aria-label="Close">
            <X size={20} />
          </button>
        </div>

        <div className="space-y-4">
          <div>
            <label className="block text-sm text-ghost mb-1.5">Name</label>
            <Input
              type="text"
              placeholder="e.g. Alice"
              value={name}
              onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
                setName(e.target.value)
                setError(null)
              }}
              autoFocus
            />
          </div>

          <div>
            <label className="block text-sm text-ghost mb-1.5">Address</label>
            <Input
              type="text"
              placeholder="tbotho://1/…"
              value={address}
              onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
                setAddress(e.target.value)
                setError(null)
              }}
              disabled={!addressEditable}
              className="font-mono text-xs"
            />
            {!addressEditable && (
              <p className="text-xs text-ghost/70 mt-1">
                The address can&apos;t be changed for an existing contact.
              </p>
            )}
          </div>

          <div>
            <label className="block text-sm text-ghost mb-1.5">
              Notes <span className="text-ghost/70">(optional)</span>
            </label>
            <textarea
              value={notes}
              onChange={(e) => {
                setNotes(e.target.value)
                setError(null)
              }}
              placeholder="Anything to remember about this contact…"
              rows={3}
              className="w-full p-3 rounded-lg bg-abyss border border-steel text-sm leading-relaxed resize-none focus:outline-none focus:ring-2 focus:ring-pulse/50 focus:border-pulse placeholder:text-ghost/50"
            />
          </div>

          {error && (
            <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
              <AlertCircle size={16} className="shrink-0" />
              <span>{error}</span>
            </div>
          )}

          <Button onClick={handleSave} disabled={busy} className="w-full justify-center">
            <Check size={16} className="mr-2" />
            {mode === 'add' ? 'Add contact' : 'Save changes'}
          </Button>

          {mode === 'edit' && onDelete && (
            confirmDelete ? (
              <div className="space-y-2">
                <p className="text-xs text-ghost text-center">
                  Remove this contact? This only deletes the local label, not any
                  payments.
                </p>
                <div className="flex gap-2">
                  <Button
                    variant="danger"
                    onClick={handleDelete}
                    disabled={busy}
                    className="flex-1 justify-center"
                  >
                    <Trash2 size={16} className="mr-2" />Delete
                  </Button>
                  <Button
                    variant="secondary"
                    onClick={() => setConfirmDelete(false)}
                    className="flex-1 justify-center"
                  >
                    Cancel
                  </Button>
                </div>
              </div>
            ) : (
              <Button
                variant="ghost"
                onClick={() => setConfirmDelete(true)}
                className="w-full justify-center text-danger"
              >
                <Trash2 size={16} className="mr-2" />Delete contact
              </Button>
            )
          )}
        </div>
      </Card>
    </div>
  )
}
