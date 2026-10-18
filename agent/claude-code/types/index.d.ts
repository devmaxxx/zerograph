// The shape of `zerograph status --json`.
export type Report = {
  index: { state: 'empty' } | { state: 'built'; doc_nodes: number; code_nodes: number; edges: number } | { state: 'unreadable'; error: string }
  tree: { state: 'in_step' | 'other_grammar' | 'behind' | 'not_walked' | 'config_unread'; files?: string[]; error?: string }
  model: { recorded: string | null; configured: string | null }
  vectors: { state: 'rows'; rows: number; owed: number } | { state: 'unreadable'; error: string } | null
  refresh: { state: 'none' | 'starting' | 'running' | 'unknown'; progress?: string; error?: string }
  last_run: string[]
  serve: { running: boolean; up_secs: number | null; log: string[] }
}

declare module 'claude-code' {
  interface PluginState {
    'zerograph-status': { report: Report | null }
  }
}
