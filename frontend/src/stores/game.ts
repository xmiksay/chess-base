// Pinia store holding the board/game state for the Analyse page. The whole
// tree/cursor state machine lives in the shared `useTreeBoard` composable
// (issue #134); the store only adds the play-vs-engine bits (`mode`/`playColor`)
// that are specific to this page.

import { defineStore } from 'pinia'
import { ref } from 'vue'
import { useTreeBoard } from '../lib/useTreeBoard'
import type { Color } from '../types'

export const useGameStore = defineStore('game', () => {
  const board = useTreeBoard()
  const mode = ref<'analyse' | 'play'>('analyse') // 'analyse' | 'play'
  const playColor = ref<Color>('white') // human's color in play mode

  return { ...board, mode, playColor }
})
