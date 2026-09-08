import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import VariableField from './VariableField.vue'
import type { TemplateVariable } from '@/lib/core/api'

const secret: TemplateVariable = { name: 'DB_PASSWORD', label: 'Database password', type: 'secret', required: true }

describe('VariableField', () => {
  it('masks a secret until the show toggle is pressed, then hides it again', async () => {
    const w = mount(VariableField, { props: { variable: secret, modelValue: 'hunter2' } })
    const input = w.get('input#var-DB_PASSWORD')
    const toggle = w.get('[data-testid="toggle-secret"]')
    expect(input.attributes('type')).toBe('password')
    expect(toggle.attributes('aria-label')).toBe('Show password')

    await toggle.trigger('click')
    expect(input.attributes('type')).toBe('text')
    expect(toggle.attributes('aria-label')).toBe('Hide password')
    expect(toggle.attributes('aria-pressed')).toBe('true')

    await toggle.trigger('click')
    expect(input.attributes('type')).toBe('password')
  })

  it('has no toggle on a non-secret field', () => {
    const w = mount(VariableField, { props: { variable: { ...secret, name: 'DB_NAME', type: 'string' }, modelValue: 'vdashboard' } })
    expect(w.find('[data-testid="toggle-secret"]').exists()).toBe(false)
  })
})
