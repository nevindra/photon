import { describe, it, expect } from 'vitest'
import config from './tailwind.config.js'

// Regression guard for the "invisible span bars" class of bug: several class-name literals live
// in plain `.ts` modules (SERVICE_PALETTE, seriesColor swatches, serviceHealth dots, format's
// severity tones). If Tailwind's content globs stop covering `.ts`, those utilities are silently
// purged from the built CSS and the waterfall bars / breakdown band / series swatches render with
// no background — a bug no unit test that only mounts components can see.

// './src/**/*.{vue,js,ts}' -> ['vue', 'js', 'ts']
function extensionsOf(glob) {
  const m = /\{([^}]+)\}$/.exec(glob)
  if (m) return m[1].split(',').map((s) => s.trim())
  const dot = glob.lastIndexOf('.')
  return dot === -1 ? [] : [glob.slice(dot + 1)]
}

describe('tailwind content globs', () => {
  const srcGlobs = config.content.filter((g) => g.startsWith('./src/'))

  it('scans src at all', () => {
    expect(srcGlobs.length).toBeGreaterThan(0)
  })

  it.each(['vue', 'js', 'ts'])('covers .%s sources', (ext) => {
    expect(srcGlobs.some((g) => extensionsOf(g).includes(ext))).toBe(true)
  })
})
