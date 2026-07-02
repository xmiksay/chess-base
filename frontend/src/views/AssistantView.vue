<script setup lang="ts">
// AI study assistant chat (issue #20, Direction B). A sidebar of chat sessions
// and a transcript view that drives the server-side agent loop: posting a
// message runs the loop until it answers, pauses for a mutating-tool approval,
// or hits the visible iteration cap. The user approves/denies each mutating call
// before it runs. The whole tool surface is the same one the MCP endpoint serves.
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { useAssistantStore } from '../stores/assistant'

const store = useAssistantStore()
const draft = ref('')
// null until the first health check resolves whether an LLM is configured.
const llmConfigured = ref<boolean | null>(null)

const session = computed(() => store.current)

function prettyInput(input: unknown): string {
  try {
    return JSON.stringify(input, null, 2)
  } catch {
    return String(input)
  }
}

async function startChat() {
  await store.create().catch(() => {})
}

async function submit() {
  const text = draft.value.trim()
  if (!text || store.sending || store.awaitingApproval) return
  draft.value = ''
  await store.sendMessage(text).catch(() => {})
}

async function approveAll() {
  await store.resolveAll(true).catch(() => {})
}
async function denyAll() {
  await store.resolveAll(false).catch(() => {})
}
async function decide(id: string, approved: boolean) {
  await store.respond({ [id]: approved }).catch(() => {})
}

onMounted(async () => {
  llmConfigured.value = (await api.health().catch(() => ({ llm: false }))).llm ?? false
  await store.refresh().catch(() => {})
})
</script>

<template>
  <div class="mx-auto flex max-w-6xl gap-6 p-6">
    <!-- Sessions sidebar -->
    <aside class="w-60 shrink-0">
      <button
        type="button"
        class="mb-3 w-full rounded bg-fg px-3 py-2 text-sm font-medium text-surface hover:opacity-90"
        @click="startChat"
      >
        + New chat
      </button>
      <ul class="space-y-1">
        <li
          v-for="s in store.sessions"
          :key="s.id"
          class="group flex items-center justify-between rounded px-2 py-1 text-sm hover:bg-surface-2"
          :class="{ 'bg-surface-2 font-semibold': s.id === session?.id }"
        >
          <button
            type="button"
            class="flex-1 truncate text-left"
            @click="store.open(s.id)"
          >
            {{ s.title }}
          </button>
          <button
            type="button"
            class="ml-2 hidden text-muted hover:text-bad group-hover:block"
            title="Delete chat"
            @click="store.remove(s.id)"
          >
            ✕
          </button>
        </li>
      </ul>
    </aside>

    <!-- Conversation -->
    <section class="flex min-h-[70vh] flex-1 flex-col">
      <p
        v-if="llmConfigured === false"
        class="mb-3 rounded border border-warn/50 bg-warn/10 px-3 py-2 text-sm text-warn"
      >
        No language model is configured. Set <code>ANTHROPIC_API_KEY</code> (or add an
        LLM provider) to use the study assistant.
      </p>

      <div
        v-if="!session"
        class="flex flex-1 items-center justify-center text-muted"
      >
        Start a chat to build studies — e.g. “build me a repertoire vs the Sicilian”.
      </div>

      <template v-else>
        <header class="mb-3 flex items-center justify-between border-b border-border pb-2">
          <h2 class="font-semibold">
            {{ session.title }}
          </h2>
          <span class="text-xs text-muted">
            {{ session.model }} · step {{ session.iterations }}/{{ session.iteration_cap }}
          </span>
        </header>

        <!-- Transcript -->
        <div class="flex-1 space-y-3 overflow-y-auto pr-1">
          <template
            v-for="(m, i) in session.messages"
            :key="i"
          >
            <!-- User -->
            <div
              v-if="m.role === 'user'"
              class="flex justify-end"
            >
              <div class="max-w-[80%] rounded-lg bg-fg px-3 py-2 text-sm text-surface">
                {{ m.text }}
              </div>
            </div>

            <!-- Assistant -->
            <div
              v-else-if="m.role === 'assistant'"
              class="flex justify-start"
            >
              <div class="max-w-[80%] space-y-2">
                <div
                  v-if="m.text"
                  class="whitespace-pre-wrap rounded-lg bg-surface-2 px-3 py-2 text-sm"
                >
                  {{ m.text }}
                </div>
                <div
                  v-for="call in m.tool_calls ?? []"
                  :key="call.id"
                  class="rounded border border-border bg-surface px-2 py-1 text-xs"
                >
                  <span class="font-mono font-semibold">⚙ {{ call.name }}</span>
                  <span
                    v-if="call.requires_approval"
                    class="ml-1 text-warn"
                  >(needs approval)</span>
                  <pre class="mt-1 overflow-x-auto text-muted">{{ prettyInput(call.input) }}</pre>
                </div>
              </div>
            </div>

            <!-- Tool results -->
            <div
              v-else
              class="flex justify-start"
            >
              <div class="max-w-[80%] space-y-1">
                <pre
                  v-for="r in m.tool_results ?? []"
                  :key="r.tool_call_id"
                  class="overflow-x-auto rounded border px-2 py-1 text-xs"
                  :class="r.is_error
                    ? 'border-bad/50 bg-bad/10 text-bad'
                    : 'border-border bg-surface-2 text-muted'"
                >{{ r.content }}</pre>
              </div>
            </div>
          </template>

          <p
            v-if="store.sending"
            class="text-sm text-muted"
          >
            Thinking…
          </p>
        </div>

        <!-- Pending approvals -->
        <div
          v-if="store.awaitingApproval"
          class="mt-3 rounded border border-warn/50 bg-warn/10 p-3"
        >
          <div class="mb-2 flex items-center justify-between">
            <span class="text-sm font-medium text-warn">
              The assistant wants to run {{ store.pending.length }} action(s):
            </span>
            <div class="flex gap-2">
              <button
                type="button"
                class="rounded bg-accent px-2 py-1 text-xs text-surface hover:opacity-90"
                @click="approveAll"
              >
                Approve all
              </button>
              <button
                type="button"
                class="rounded bg-surface-2 px-2 py-1 text-xs hover:bg-surface-2"
                @click="denyAll"
              >
                Deny all
              </button>
            </div>
          </div>
          <ul class="space-y-1">
            <li
              v-for="call in store.pending"
              :key="call.id"
              class="flex items-center justify-between rounded bg-surface px-2 py-1 text-xs"
            >
              <span class="font-mono">{{ call.name }}</span>
              <span class="flex gap-1">
                <button
                  type="button"
                  class="text-accent hover:underline"
                  @click="decide(call.id, true)"
                >
                  approve
                </button>
                <button
                  type="button"
                  class="text-bad hover:underline"
                  @click="decide(call.id, false)"
                >
                  deny
                </button>
              </span>
            </li>
          </ul>
        </div>

        <!-- Composer -->
        <form
          class="mt-3 flex gap-2"
          @submit.prevent="submit"
        >
          <textarea
            v-model="draft"
            rows="2"
            placeholder="Ask the assistant to analyse a position or build a study…"
            class="flex-1 resize-none rounded border border-border px-3 py-2 text-sm focus:border-border focus:outline-none"
            :disabled="store.awaitingApproval"
            @keydown.enter.exact.prevent="submit"
          />
          <button
            type="submit"
            class="self-end rounded bg-fg px-4 py-2 text-sm font-medium text-surface hover:opacity-90 disabled:opacity-50"
            :disabled="store.sending || store.awaitingApproval || !draft.trim()"
          >
            Send
          </button>
        </form>
        <p
          v-if="store.awaitingApproval"
          class="mt-1 text-xs text-warn"
        >
          Resolve the pending action(s) above before sending another message.
        </p>
        <p
          v-if="store.error"
          class="mt-1 text-xs text-bad"
        >
          {{ store.error }}
        </p>
      </template>
    </section>
  </div>
</template>
