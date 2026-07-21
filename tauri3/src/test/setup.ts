import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach, vi } from 'vitest'

afterEach(cleanup)

Object.defineProperty(window, 'confirm', { configurable: true, value: vi.fn(() => true) })
Object.defineProperty(window, 'prompt', { configurable: true, value: vi.fn(() => null) })
