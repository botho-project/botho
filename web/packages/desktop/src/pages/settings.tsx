import { Layout } from '../components/layout'
import { Card, CardHeader, CardTitle, CardContent, Button, Input } from '@botho/ui'
import { useState } from 'react'
import {
  Server,
  Cpu,
  Shield,
  Download,
} from 'lucide-react'

export function SettingsPage() {
  const [minerThreads, setMinerThreads] = useState('4')
  const [dataDir, setDataDir] = useState('~/.botho')

  return (
    <Layout title="Settings" subtitle="Configure your node">
      <div className="max-w-2xl space-y-6">
        {/* Node settings */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Server className="h-4 w-4 text-[--color-pulse]" />
              <CardTitle>Node Configuration</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="space-y-4">
            <div>
              <label className="text-sm text-[--color-ghost]">Data Directory</label>
              <Input
                value={dataDir}
                onChange={(e) => setDataDir(e.target.value)}
                className="mt-1"
              />
              <p className="mt-1 text-xs text-[--color-dim]">
                Location where blockchain data and wallet files are stored
              </p>
            </div>
          </CardContent>
        </Card>

        {/* Mining settings */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Cpu className="h-4 w-4 text-[--color-pulse]" />
              <CardTitle>Mining Configuration</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="space-y-4">
            <div>
              <label className="text-sm text-[--color-ghost]">Mining Threads</label>
              <Input
                type="number"
                value={minerThreads}
                onChange={(e) => setMinerThreads(e.target.value)}
                min="1"
                max="64"
                className="mt-1"
              />
              <p className="mt-1 text-xs text-[--color-dim]">
                Number of CPU threads to use for mining (1-64)
              </p>
            </div>
          </CardContent>
        </Card>

        {/* Privacy settings */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Shield className="h-4 w-4 text-[--color-pulse]" />
              <CardTitle>Privacy Settings</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-[--color-light]">Default Transaction Privacy</p>
                <p className="text-xs text-[--color-dim]">
                  Use hidden transactions by default (higher fees)
                </p>
              </div>
              <Button variant="secondary" size="sm">
                Hidden
              </Button>
            </div>
          </CardContent>
        </Card>

        {/* Export/Import */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Download className="h-4 w-4 text-[--color-pulse]" />
              <CardTitle>Backup & Restore</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex gap-4">
              <Button variant="secondary" className="flex-1">
                Export Wallet
              </Button>
              <Button variant="secondary" className="flex-1">
                Import Wallet
              </Button>
            </div>
            <p className="text-xs text-[--color-dim]">
              Export your wallet seed phrase or import an existing wallet.
              Keep your seed phrase safe and never share it.
            </p>
          </CardContent>
        </Card>

        {/* Save button */}
        <div className="flex justify-end">
          <Button>Save Settings</Button>
        </div>
      </div>
    </Layout>
  )
}
