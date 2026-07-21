export type Diagnostic = {
  status: string
  devtoolsUrl: string
  pageTitle: string
  pageUrl: string
  nimAccount: string
  detail: string
}

export type RuntimeControlProps = {
  diagnostic: Diagnostic | null
  loading: boolean
  refresh: () => Promise<void>
  setError: (message: string) => void
}
