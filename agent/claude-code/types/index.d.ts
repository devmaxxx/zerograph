// The shape of `zerograph status --json`.
export type Report = {
  index: { state: 'empty' } | { state: 'built'; doc_nodes: number; code_nodes: number; edges: number } | { state: 'unreadable'; error: string }
  tree: { state: 'in_step' | 'other_grammar' | 'newer_grammar' | 'behind' | 'not_walked' | 'config_unread'; files?: string[]; error?: string; grammar?: number; release?: string }
  model: { recorded: string | null; configured: string | null }
  vectors: { state: 'rows'; rows: number; owed: number } | { state: 'unreadable'; error: string } | null
  refresh: { state: 'none' | 'starting' | 'running' | 'unknown'; progress?: string; error?: string }
  last_run: string[]
  // `socket` is absent from a zerograph that predates it.
  serve: { running: boolean; socket?: string; up_secs: number | null; log: string[] }
  // A newer release the CLI's daily check cached; absent from a zerograph that predates it.
  update?: string | null
}

declare module 'claude-code' {
  interface PluginState {
    'zerograph': { report: Report | null; full: { report: Report | null; takenAt: number } | null }
  }
}
