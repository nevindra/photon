import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HostStatTiles from './HostStatTiles.vue'

const series = (labels: Record<string, string>, v: number) => ({
  labels,
  points: [{ t: '0', v }],
})
const q = (list: unknown[]) => ({ data: { value: { series: list } } })

const res = {
  cpu: q([series({ cpu: 'total' }, 0.18), series({ cpu: '0' }, 0.5)]),
  memory: q([series({ 'host.name': 'h' }, 0.48)]),
  disk: q([series({ mountpoint: '/' }, 0.67), series({ mountpoint: '/boot/efi' }, 0.04)]),
  network: q([series({ direction: 'receive' }, 1_500_000), series({ direction: 'transmit' }, 600_000)]),
  load: q([]),
  gpu: q([series({ gpu: '0' }, 0.43)]),
  gpuMemory: q([]),
  gpuTemp: q([series({ gpu: '0' }, 61)]),
  gpuPower: q([]),
} as never

describe('HostStatTiles', () => {
  it('derives current values from the last series points', () => {
    const w = mount(HostStatTiles, {
      props: { res, totalRamBytes: 32 * 1024 ** 3, hasGpu: true },
    })
    const text = w.text()
    expect(text).toContain('18%')            // cpu total (not the 50% core)
    expect(text).toContain('48%')            // memory
    expect(text).toContain('67%')            // worst mountpoint
    expect(text).toContain('/')              // its label
    expect(text).toContain('2.0 MB/s')       // rx+tx combined
    expect(text).toContain('43%')            // gpu util
    expect(text).toContain('61°C')           // gpu temp
  })
  it('hides GPU tiles when hasGpu is false', () => {
    const w = mount(HostStatTiles, { props: { res, totalRamBytes: null, hasGpu: false } })
    expect(w.text()).not.toContain('61°C')
  })
})
