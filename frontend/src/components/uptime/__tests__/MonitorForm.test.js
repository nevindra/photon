import { describe, it, expect, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import MonitorForm from '@/components/uptime/MonitorForm.vue'

afterEach(() => {
  document.body.innerHTML = ''
})

const tick = () => new Promise((r) => setTimeout(r))

const open = async (props = {}) => {
  const w = mount(MonitorForm, { props: { modelValue: true, ...props }, attachTo: document.body })
  await tick()
  return w
}

const submit = () => document.body.querySelector('form').dispatchEvent(new Event('submit', { cancelable: true, bubbles: true }))

describe('MonitorForm', () => {
  it('renders the sheet with the new Select and Switch controls', async () => {
    await open()
    expect(document.body.textContent).toContain('New monitor')
    // section grouping is present
    expect(document.body.textContent).toContain('HTTP request')
    // method is a real control, not a free-text guess — its default is exposed
    expect(document.body.querySelector('#monitor-method')).toBeTruthy()
    expect(document.body.querySelector('#monitor-ignore-tls')).toBeTruthy()
  })

  it('emits an HTTP payload carrying the enum method + toggle defaults', async () => {
    const w = await open()
    submit()
    await tick()
    const body = w.emitted('save')?.at(-1)?.[0]
    expect(body).toMatchObject({
      type: 'http',
      http_method: 'GET',
      expect_status: '2xx',
      ignore_tls: false,
      follow_redirects: true,
      interval_secs: 60,
    })
  })

  it('omits HTTP-only fields for a non-HTTP monitor', async () => {
    const w = await open({ monitor: { name: 'db', type: 'tcp', target: 'db:5432', interval_secs: 30 } })
    submit()
    await tick()
    const body = w.emitted('save')?.at(-1)?.[0]
    expect(body.type).toBe('tcp')
    expect(body).not.toHaveProperty('http_method')
    expect(body).not.toHaveProperty('ignore_tls')
  })
})
