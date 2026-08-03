import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HostStatTiles from './HostStatTiles.vue'
import { StatTile } from '@/components/ui/stat-tile'

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
  it('pairs each split resource with its across-group mean and names the worst group', () => {
    const multi = {
      ...(res as object),
      gpu: q([
        series({ gpu: '0' }, 0.01),
        series({ gpu: '1' }, 0.02),
        series({ gpu: '2' }, 0.76),
        series({ gpu: '3' }, 0.01),
      ]),
      gpuTemp: q([series({ gpu: '0' }, 44), series({ gpu: '2' }, 74)]),
    } as never
    const text = mount(HostStatTiles, {
      props: { res: multi, totalRamBytes: null, hasGpu: true },
    }).text()
    // The headline stays the busiest device — but never alone.
    expect(text).toContain('76%')
    expect(text).toContain('max · 20% avg')   // (0.01+0.02+0.76+0.01)/4
    expect(text).toContain('gpu 2 of 4')
    // Same treatment for temperature and for disk, from the same helper.
    expect(text).toContain('74°C')
    expect(text).toContain('max · 59°C avg')
    expect(text).toContain('67%')
    expect(text).toContain('max · 36% avg')   // (0.67+0.04)/2
    expect(text).toContain('/ of 2 mounts')
  })
  it('drops the pair on a single-group resource, where the mean just repeats the max', () => {
    // `res` has ONE GPU but TWO mountpoints, so assert per tile: the GPU tiles must lose the pair
    // while the Disk tile keeps it. A whole-card text scan can't tell those apart.
    const tiles = mount(HostStatTiles, { props: { res, totalRamBytes: null, hasGpu: true } })
      .findAllComponents(StatTile)
    const byLabel = Object.fromEntries(tiles.map((t) => [t.props('label'), t]))

    expect(byLabel['GPU'].props('value')).toBe('43%')
    expect(byLabel['GPU'].props('secondary')).toBeUndefined()
    expect(byLabel['GPU'].props('sub')).toBe('gpu 0')       // named, but not "of 1"
    expect(byLabel['GPU temp'].props('secondary')).toBeUndefined()
    expect(byLabel['Disk'].props('secondary')).toBe('max · 36% avg')
    // CPU and Memory are never split — no pair, ever.
    expect(byLabel['CPU'].props('secondary')).toBeUndefined()
    expect(byLabel['Memory'].props('secondary')).toBeUndefined()
  })
  it('hides GPU tiles when hasGpu is false', () => {
    const w = mount(HostStatTiles, { props: { res, totalRamBytes: null, hasGpu: false } })
    expect(w.text()).not.toContain('61°C')
  })
})
