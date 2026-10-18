import test from 'node:test'
import assert from 'node:assert/strict'
import {
  bestModelForRole,
  compatibleModels,
  hardwareTier,
  isInstalledModel,
} from '../src/shared/model-catalog.ts'
import type { SystemProfile } from '../src/shared/types.ts'

function profile(ramGb: number, availableModelMemoryGb: number, overrides: Partial<SystemProfile> = {}): SystemProfile {
  return {
    platform: 'test',
    arch: 'arm64',
    cpu: 'Test CPU',
    cpuCores: 8,
    ramGb,
    gpu: 'Test GPU',
    gpuVendor: 'apple',
    vramGb: null,
    unifiedMemory: true,
    availableModelMemoryGb,
    tier: hardwareTier(ramGb, availableModelMemoryGb),
    ...overrides,
  }
}

test('hardware tiers cover low through workstation systems', () => {
  assert.equal(hardwareTier(8, 5), 'basic')
  assert.equal(hardwareTier(16, 10), 'standard')
  assert.equal(hardwareTier(24, 16), 'performance')
  assert.equal(hardwareTier(32, 25), 'workstation')
})

test('catalog excludes models above the conservative memory budget', () => {
  const catalog = compatibleModels(profile(8, 5))
  assert.equal(catalog.find((model) => model.name === 'qwen3-vl:8b')?.compatible, false)
  assert.equal(catalog.find((model) => model.name === 'qwen3:1.7b')?.compatible, true)
})

test('installed tags match catalog families and rank the best compatible role', () => {
  const installed = ['qwen3-vl:4b-q4_K_M', 'qwen3:4b']
  assert.equal(isInstalledModel('qwen3-vl:4b', installed), true)
  assert.equal(bestModelForRole('vision', profile(16, 12), installed, true), 'qwen3-vl:4b')
  assert.equal(bestModelForRole('text', profile(16, 12), installed, true), 'qwen3:4b')
})

test('dedicated vision models are not offered as text defaults', () => {
  const installed = ['qwen3-vl:8b']
  assert.equal(bestModelForRole('text', profile(32, 25), installed), '')
})
