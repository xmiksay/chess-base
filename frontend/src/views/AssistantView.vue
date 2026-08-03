<script setup lang="ts">
// AI study assistant (issue #198): a streaming chat over the assistant
// WebSocket. Sidebar of conversations (rename inline, delete with confirm,
// live dot) + the transcript pane rendered by AssistantTranscript. The
// composer starts a new conversation when none is open; Stop cancels the
// in-flight turn. Approvals/questions resolve through the store's InMsg
// actions; throttle/gap banners and the usage footer come from stream state.
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import AssistantTranscript from '../components/AssistantTranscript.vue'
import ModelSelect from '../components/ModelSelect.vue'
import { api } from '../api'
import { effectiveDefault } from '../lib/providers'
import type { ModelChoice } from '../lib/providers'
import type { ProviderInfo } from '../types'

const store = useAssistantStore()
const draft = ref('')
const renamingRoot = ref<string | null>(null)
const renameDraft = ref('')

// Per-session model choice (issue #215): `newChoice` seeds the next new
// conversation (null = the default row); the header picker switches the open
// one mid-session. Provider rows are fetched once and shared by both pickers.
const newChoice = ref<ModelChoice | null>(null)
const providerRows = ref<ProviderInfo[]>([])

/** The open conversation's model, folded from the event stream. */
const currentModel = computed(() => store.transcript.model)

function switchModel(choice: ModelChoice | null) {
  // Picking the "(default)" entry emits null; resolve it to the concrete
  // default row so the engine gets an explicit (provider, model) pair.
  const target =
    choice ??
    (() => {
      const row = effectiveDefault(providerRows.value)
      return row ? { provider: row.name, model: row.model } : null
    })()
  if (target) store.setModel(target.provider, target.model)
}

const STATUS_LABEL: Record<string, string> = {
  thinking: 'Thinking…',
  working: 'Running tools…',
  waiting_agent: 'Waiting for a sub-agent…',
  waiting_approval: 'Waiting for your approval',
  waiting_answer: 'Waiting for your answer',
  paused: 'Paused',
  error: 'The last turn ended with an error',
}
const statusLine = computed(() =>
  store.transcript.status ? (STATUS_LABEL[store.transcript.status] ?? null) : null,
)

const title = computed(() => store.current?.name || (store.currentRoot ? 'Conversation' : 'New chat'))

/** Relative "3m ago" from a backend timestamp (naive UTC unless zoned). */
function relTime(ts: string): string {
  const zoned = /Z$|[+-]\d\d:\d\d$/.test(ts) ? ts : `${ts}Z`
  const ms = Date.now() - new Date(zoned).getTime()
  if (!Number.isFinite(ms)) return ''
  const min = Math.max(0, Math.round(ms / 60000))
  if (min < 1) return 'now'
  if (min < 60) return `${min}m ago`
  const h = Math.round(min / 60)
  if (h < 24) return `${h}h ago`
  return `${Math.round(h / 24)}d ago`
}

function submit() {
  const text = draft.value.trim()
  if (!text) return
  draft.value = ''
  if (store.currentRoot) store.send(text)
  else store.newSession(text, null, newChoice.value)
}

function startRename(root: string, name: string | null) {
  renamingRoot.value = root
  renameDraft.value = name ?? ''
}

function finishRename() {
  const root = renamingRoot.value
  const name = renameDraft.value.trim()
  if (root && name) store.rename(root, name)
  renamingRoot.value = null
}

function confirmDelete(root: string) {
  if (window.confirm('Delete this conversation?')) store.remove(root)
}

onMounted(async () => {
  store.connect()
  try {
    providerRows.value = await api.providers.list()
  } catch {
    providerRows.value = [] // the pickers render their disabled placeholder
  }
})
onUnmounted(() => store.disconnect())
</script>

<template>
  <div class="mx-auto flex max-w-6xl gap-6 p-6">
    <!-- Conversation sidebar -->
    <aside class="w-64 shrink-0">
      <button
        type="button"
        class="mb-3 w-full rounded bg-fg px-3 py-2 text-sm font-medium text-surface hover:opacity-90"
        @click="store.deselect()"
      >
        + New chat
      </button>
      <ul class="space-y-1">
        <li
          v-for="s in store.sessions"
          :key="s.root_session_id"
          class="group rounded px-2 py-1 text-sm hover:bg-surface-2"
          :class="{ 'bg-surface-2': s.root_session_id === store.currentRoot }"
        >
          <form
            v-if="renamingRoot === s.root_session_id"
            @submit.prevent="finishRename"
          >
            <input
              v-model="renameDraft"
              class="w-full rounded border border-border bg-surface px-1 py-0.5 text-sm"
              autofocus
              @blur="finishRename"
              @keydown.esc="renamingRoot = null"
            >
          </form>
          <div
            v-else
            class="flex items-center gap-2"
          >
            <span
              class="h-2 w-2 shrink-0 rounded-full"
              :class="s.live ? 'bg-good' : 'bg-border'"
              :title="s.live ? 'live' : 'idle'"
            />
            <button
              type="button"
              class="min-w-0 flex-1 text-left"
              @click="store.open(s.root_session_id)"
            >
              <span class="block truncate">{{ s.name || 'Untitled chat' }}</span>
              <span class="block text-xs text-muted">{{ relTime(s.last_active) }}</span>
            </button>
            <button
              type="button"
              class="hidden text-xs text-muted hover:text-fg group-hover:block"
              title="Rename"
              @click="startRename(s.root_session_id, s.name)"
            >
              ✎
            </button>
            <button
              type="button"
              class="hidden text-xs text-muted hover:text-bad group-hover:block"
              title="Delete"
              @click="confirmDelete(s.root_session_id)"
            >
              ✕
            </button>
          </div>
        </li>
      </ul>
    </aside>

    <!-- Chat pane -->
    <section class="flex min-h-[75vh] flex-1 flex-col">
      <!-- Unavailable: engine off (WS refused) or no provider configured. -->
      <p
        v-if="store.unavailable"
        class="mb-3 rounded border border-warn/50 bg-warn/10 px-3 py-2 text-sm text-warn"
        data-test="unavailable"
      >
        The assistant is unavailable — the agent engine is off or no AI provider is
        configured.
        <router-link
          to="/settings"
          class="underline"
        >
          Configure a provider in Settings
        </router-link>.
      </p>

      <p
        v-if="store.throttled"
        class="mb-2 rounded border border-warn/50 bg-warn/10 px-3 py-1 text-xs text-warn"
      >
        The AI provider is throttling requests — replies may pause.
      </p>
      <p
        v-if="store.gapped"
        class="mb-2 rounded border border-warn/50 bg-warn/10 px-3 py-1 text-xs text-warn"
      >
        Some events were dropped — reopen the conversation to reload it.
      </p>

      <header class="mb-3 flex items-center justify-between border-b border-border pb-2">
        <h2 class="font-semibold">
          {{ title }}
        </h2>
        <div class="flex items-center gap-2">
          <!-- Mid-session model switch: the value tracks the transcript's
               folded model_changed events, the change ships a set_model. -->
          <span
            v-if="store.currentRoot"
            :title="store.streaming ? 'Applies after the current reply' : undefined"
            data-test="session-model"
          >
            <ModelSelect
              :model-value="currentModel"
              :providers="providerRows"
              @update:model-value="switchModel"
            />
          </span>
          <span
            v-if="store.current"
            class="text-xs text-muted"
          >{{ store.current.agent }}</span>
        </div>
      </header>

      <!-- Transcript -->
      <div class="flex-1 overflow-y-auto pr-1">
        <div
          v-if="!store.currentRoot && store.transcript.items.length === 0"
          class="flex h-full items-center justify-center text-center text-muted"
        >
          Start a chat to analyse games or build studies —<br>
          e.g. “build me a repertoire vs the Sicilian”.
        </div>
        <AssistantTranscript
          v-else
          :transcript="store.transcript"
          @approve="(id, scope) => store.approve(id, scope)"
          @reject="(id) => store.reject(id)"
          @answer="(id, answers) => store.answerQuestion(id, answers)"
        />
      </div>

      <p
        v-if="statusLine"
        class="mt-2 text-xs text-muted"
        data-test="status"
      >
        {{ statusLine }}
      </p>
      <p
        v-if="store.error"
        class="mt-1 text-xs text-bad"
      >
        {{ store.error }}
      </p>

      <!-- Composer -->
      <form
        class="mt-3 flex gap-2"
        @submit.prevent="submit"
      >
        <!-- New-conversation model choice; null keeps the default row. -->
        <span
          v-if="!store.currentRoot"
          class="self-end"
          data-test="new-model"
        >
          <ModelSelect
            v-model="newChoice"
            :providers="providerRows"
          />
        </span>
        <textarea
          v-model="draft"
          rows="2"
          :placeholder="store.currentRoot
            ? 'Reply… (Enter to send)'
            : 'Start a new chat… (Enter to send)'"
          class="flex-1 resize-none rounded border border-border bg-surface px-3 py-2 text-sm focus:outline-none"
          @keydown.enter.exact.prevent="submit"
        />
        <button
          v-if="store.streaming"
          type="button"
          class="self-end rounded border border-border px-4 py-2 text-sm hover:bg-surface-2"
          @click="store.stop()"
        >
          Stop
        </button>
        <button
          type="submit"
          class="self-end rounded bg-fg px-4 py-2 text-sm font-medium text-surface hover:opacity-90 disabled:opacity-50"
          :disabled="!draft.trim() || !store.connected"
        >
          Send
        </button>
      </form>

      <!-- Usage footer -->
      <p
        v-if="store.transcript.usage"
        class="mt-1 text-right text-xs text-muted"
        data-test="usage"
      >
        {{ store.transcript.usage.input_tokens.toLocaleString() }} in ·
        {{ store.transcript.usage.output_tokens.toLocaleString() }} out
        <template v-if="store.transcript.usage.cost_usd != null">
          · ${{ store.transcript.usage.cost_usd.toFixed(4) }}
        </template>
      </p>
    </section>
  </div>
</template>
