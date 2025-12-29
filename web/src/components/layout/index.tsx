import type { ReactNode } from 'react'
import { Sidebar } from './sidebar'
import { Header } from './header'

interface LayoutProps {
  children: ReactNode
  title: string
  subtitle?: string
}

export function Layout({ children, title, subtitle }: LayoutProps) {
  return (
    <div className="min-h-screen">
      <Sidebar />
      <div className="pl-64">
        <Header title={title} subtitle={subtitle} />
        <main className="grid-pattern min-h-[calc(100vh-4rem)] p-6">
          {children}
        </main>
      </div>
    </div>
  )
}

export { Sidebar } from './sidebar'
export { Header } from './header'
