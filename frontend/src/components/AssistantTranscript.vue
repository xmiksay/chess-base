<script setup lang="ts">
// Renders the assistant conversation transcript (issue #198): user/assistant
// bubbles (with a live streaming cursor and a collapsible reasoning block),
// tool chips, approval cards (Allow once / this session / always / Reject),
// model-driven question cards, error notices and compaction dividers. Pure
// display over lib/assistantStream's TranscriptState; decisions bubble up.
import { reactive } from 'vue'
import type { QuestionCard, TranscriptState } from '../lib/assistantStream'

defineProps<{ transcript: TranscriptState }>()

const emit = defineEmits<{
  approve: [requestId: string, scope: 'once' | 'session' | 'always']
  reject: [requestId: string]
  answer: [requestId: string, answers: string[][]]
}>()

// Question-card draft state, keyed by `${requestId}:${questionIndex}`.
const picks = reactive<Record<string, string[]>>({})
const others = reactive<Record<string, string>>({})

function key(card: QuestionCard, qi: number) {
  return `${card.requestId}:${qi}`
}

function toggle(k: string, label: string, multi: boolean | undefined) {
  const cur = picks[k] ?? []
  if (multi) picks[k] = cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label]
  else picks[k] = [label]
}

function picked(k: string, label: string) {
  return (picks[k] ?? []).includes(label)
}

/** One inner array per question: the picked labels plus any free-text answer. */
function draftAnswers(card: QuestionCard): string[][] {
  return card.questions.map((_q, qi) => {
    const k = key(card, qi)
    const other = (others[k] ?? '').trim()
    const chosen = picks[k] ?? []
    return other ? [...chosen, other] : chosen
  })
}

function canSubmit(card: QuestionCard) {
  return draftAnswers(card).every((a) => a.length > 0)
}

function submitAnswers(card: QuestionCard) {
  if (canSubmit(card)) emit('answer', card.requestId, draftAnswers(card))
}

const CHIP_ICON = { running: '⏳', done: '✓', error: '✗' } as const
</script>

<template>
  <div class="space-y-3">
    <template
      v-for="(item, i) in transcript.items"
      :key="i"
    >
      <!-- User bubble -->
      <div
        v-if="item.type === 'user'"
        class="flex justify-end"
      >
        <div class="max-w-[80%] whitespace-pre-wrap rounded-lg bg-fg px-3 py-2 text-sm text-surface">
          {{ item.text }}
        </div>
      </div>

      <!-- Assistant bubble -->
      <div
        v-else-if="item.type === 'assistant'"
        class="flex justify-start"
      >
        <div class="max-w-[80%] space-y-1">
          <details
            v-if="item.reasoning"
            class="rounded border border-border px-2 py-1 text-xs text-muted"
          >
            <summary class="cursor-pointer select-none">
              Thinking
            </summary>
            <pre class="mt-1 whitespace-pre-wrap font-sans">{{ item.reasoning }}</pre>
          </details>
          <div
            v-if="item.text || item.streaming"
            class="whitespace-pre-wrap rounded-lg bg-surface-2 px-3 py-2 text-sm"
          >
            {{ item.text }}<span
              v-if="item.streaming"
              class="animate-pulse"
            >▍</span>
          </div>
        </div>
      </div>

      <!-- Tool chip -->
      <div
        v-else-if="item.type === 'tool'"
        class="flex justify-start"
      >
        <span
          class="inline-flex items-center gap-1 rounded-full border border-border bg-surface px-2 py-0.5 font-mono text-xs"
          :class="item.state === 'error' ? 'text-bad' : item.state === 'done' ? 'text-muted' : ''"
        >
          {{ CHIP_ICON[item.state] }} {{ item.tool }}
        </span>
      </div>

      <!-- Approval card -->
      <div
        v-else-if="item.type === 'approval'"
        class="rounded border p-3"
        :class="item.resolved ? 'border-border' : 'border-warn/50 bg-warn/10'"
      >
        <div class="mb-1 flex items-center justify-between text-sm">
          <span class="font-medium">
            <span
              v-if="!item.resolved"
              class="text-warn"
            >Approval needed:</span>
            <span class="ml-1 font-mono">{{ item.tool }}</span>
          </span>
          <span
            v-if="item.resolved"
            class="text-xs"
            :class="item.resolved === 'approved' ? 'text-good' : 'text-bad'"
          >
            {{ item.resolved }}
          </span>
        </div>
        <pre class="max-h-48 overflow-auto rounded bg-surface px-2 py-1 text-xs text-muted">{{ item.prettyInput }}</pre>
        <div
          v-if="!item.resolved"
          class="mt-2 flex flex-wrap gap-2"
        >
          <button
            type="button"
            class="rounded bg-fg px-2 py-1 text-xs font-medium text-surface hover:opacity-90"
            @click="emit('approve', item.requestId, 'once')"
          >
            Allow once
          </button>
          <button
            type="button"
            class="rounded bg-surface-2 px-2 py-1 text-xs hover:opacity-80"
            @click="emit('approve', item.requestId, 'session')"
          >
            Allow this session
          </button>
          <button
            type="button"
            class="rounded bg-surface-2 px-2 py-1 text-xs hover:opacity-80"
            @click="emit('approve', item.requestId, 'always')"
          >
            Always allow
          </button>
          <button
            type="button"
            class="rounded px-2 py-1 text-xs text-bad hover:underline"
            @click="emit('reject', item.requestId)"
          >
            Reject
          </button>
        </div>
      </div>

      <!-- Question card -->
      <div
        v-else-if="item.type === 'question'"
        class="rounded border p-3"
        :class="item.resolved ? 'border-border' : 'border-accent/50'"
      >
        <div
          v-for="(q, qi) in item.questions"
          :key="qi"
          class="mb-2"
        >
          <p class="mb-1 text-sm font-medium">
            {{ q.question }}
          </p>
          <div
            v-if="!item.resolved"
            class="flex flex-col gap-1"
          >
            <label
              v-for="opt in q.options"
              :key="opt.label"
              class="flex items-start gap-2 text-sm"
            >
              <input
                :type="q.multi_select ? 'checkbox' : 'radio'"
                :checked="picked(key(item, qi), opt.label)"
                class="mt-1"
                @change="toggle(key(item, qi), opt.label, q.multi_select)"
              >
              <span>
                {{ opt.label }}
                <span
                  v-if="opt.description"
                  class="block text-xs text-muted"
                >{{ opt.description }}</span>
              </span>
            </label>
            <input
              v-model="others[key(item, qi)]"
              type="text"
              placeholder="Other…"
              class="mt-1 rounded border border-border bg-surface px-2 py-1 text-sm"
            >
          </div>
        </div>
        <button
          v-if="!item.resolved"
          type="button"
          class="rounded bg-fg px-2 py-1 text-xs font-medium text-surface hover:opacity-90 disabled:opacity-50"
          :disabled="!canSubmit(item)"
          @click="submitAnswers(item)"
        >
          Answer
        </button>
        <p
          v-else
          class="text-xs text-muted"
        >
          answered
        </p>
      </div>

      <!-- Error notice -->
      <p
        v-else-if="item.type === 'error'"
        class="rounded border border-bad/50 bg-bad/10 px-3 py-2 text-sm text-bad"
      >
        {{ item.message }}
      </p>

      <!-- Divider (compaction) -->
      <div
        v-else-if="item.type === 'divider'"
        class="flex items-center gap-2 text-xs text-muted"
      >
        <span class="h-px flex-1 bg-border" />
        {{ item.label }}
        <span class="h-px flex-1 bg-border" />
      </div>
    </template>
  </div>
</template>
