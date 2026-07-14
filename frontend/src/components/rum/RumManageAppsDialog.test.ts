import { describe, it, expect, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { VueQueryPlugin } from '@tanstack/vue-query'
import RumManageAppsDialog from './RumManageAppsDialog.vue'

// The dialog's content lives inside a reka-ui `DialogPortal`, which teleports to
// `document.body` only once its internal `useMounted()` flag flips post-mount — mirrors
// MonitorDetailDialog.test.js's convention: attach to the real DOM, wait a tick for the
// teleport + Presence to settle, then assert against `document.body` (not the wrapper root,
// which only retains the teleport's start/end comment anchors).
const apps = [
  { name: 'web', key: 'pk_live_web', allowed_origins: ['https://web.example.com'], sample_rate: 1, rate_limit: 5000, created_at: 0 },
]

function mountDialog() {
  return mount(RumManageAppsDialog, {
    props: { open: true, apps },
    attachTo: document.body,
    global: { plugins: [VueQueryPlugin] },
  })
}

afterEach(() => {
  document.body.innerHTML = ''
})

describe('RumManageAppsDialog', () => {
  it('lists existing apps with their keys and origins', async () => {
    mountDialog()
    await new Promise((r) => setTimeout(r))
    expect(document.body.textContent).toContain('web')
    expect(document.body.textContent).toContain('pk_live_web')
    // Origins render into a `<textarea>` — its content is the DOM `value` property, not a
    // text node, so it doesn't show up in `textContent`.
    const originsEl = document.body.querySelector('[data-testid="app-origins"]') as HTMLTextAreaElement
    expect(originsEl?.value).toContain('https://web.example.com')
  })

  it('shows a create form with name + origins inputs', async () => {
    mountDialog()
    await new Promise((r) => setTimeout(r))
    expect(document.body.querySelector('[data-testid="new-app-name"]')).toBeTruthy()
    expect(document.body.querySelector('[data-testid="new-app-origins"]')).toBeTruthy()
  })
})
