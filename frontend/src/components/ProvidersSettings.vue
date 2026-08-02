<script setup lang="ts">
// AI provider registry (issue #20, per-user since #198): list, add/edit and
// delete the LLM providers the assistant can use. Keys are write-only — the
// form's key field stays blank on edit and a blank value keeps the stored key
// (`has_key` badges what's on file). Global rows are admin-only.
import { onMounted, reactive, ref } from 'vue'
import { api } from '../api'
import type { ProviderInfo } from '../types'

const WIRES = ['anthropic', 'openai', 'gemini'] as const

const providers = ref<ProviderInfo[]>([])
const isAdmin = ref(false)
const error = ref<string | null>(null)
// null = collapsed; 0 = adding; an id = editing that row.
const editing = ref<number | null>(null)

const blank = () => ({
  name: '',
  wire: 'anthropic',
  model: '',
  base_url: '',
  api_key: '',
  is_default: false,
  is_global: false,
})
const form = reactive(blank())

async function load() {
  try {
    providers.value = await api.providers.list()
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  }
}

function startAdd() {
  Object.assign(form, blank())
  editing.value = 0
  error.value = null
}

function startEdit(p: ProviderInfo) {
  Object.assign(form, {
    name: p.name,
    wire: p.wire,
    model: p.model,
    base_url: p.base_url ?? '',
    api_key: '', // never displayed; blank keeps the stored key
    is_default: p.is_default,
    is_global: p.is_global,
  })
  editing.value = p.id
  error.value = null
}

async function save() {
  error.value = null
  try {
    await api.providers.upsert({
      name: form.name.trim(),
      wire: form.wire,
      model: form.model.trim(),
      base_url: form.base_url.trim() || null,
      // Blank ⇒ omit so the stored key survives the edit.
      ...(form.api_key.trim() ? { api_key: form.api_key.trim() } : {}),
      is_default: form.is_default,
      is_global: form.is_global,
    })
    editing.value = null
    await load()
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  }
}

async function remove(p: ProviderInfo) {
  if (!window.confirm(`Delete provider “${p.name}”?`)) return
  try {
    await api.providers.remove(p.id)
    await load()
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  }
}

onMounted(async () => {
  await load()
  try {
    isAdmin.value = (await api.whoami()).is_admin === true
  } catch {
    // Unknown identity: just hide the admin-only global toggle.
  }
})
</script>

<template>
  <section class="rounded border border-border p-4">
    <div class="mb-1 flex items-center justify-between">
      <h2 class="text-lg font-semibold">
        AI providers
      </h2>
      <button
        type="button"
        class="rounded bg-fg px-2 py-1 text-xs font-medium text-surface hover:opacity-90"
        data-test="provider-add"
        @click="startAdd"
      >
        + Add provider
      </button>
    </div>
    <p class="mb-3 text-sm text-muted">
      Language models for the study assistant. API keys are stored server-side and
      never shown again.
    </p>

    <p
      v-if="error"
      class="mb-2 text-sm text-bad"
      data-test="provider-error"
    >
      {{ error }}
    </p>

    <ul
      v-if="providers.length"
      class="mb-3 divide-y divide-border"
    >
      <li
        v-for="p in providers"
        :key="p.id"
        class="flex items-center gap-2 py-2 text-sm"
        data-test="provider-row"
      >
        <span
          v-if="p.is_default"
          title="Default provider"
        >★</span>
        <span class="font-medium">{{ p.name }}</span>
        <span class="text-xs text-muted">{{ p.wire }} · {{ p.model }}</span>
        <span
          v-if="p.has_key"
          class="rounded bg-good/10 px-1.5 py-0.5 text-xs text-good"
        >key set</span>
        <span
          v-else
          class="rounded bg-warn/10 px-1.5 py-0.5 text-xs text-warn"
        >no key</span>
        <span
          v-if="p.is_global"
          class="rounded bg-surface-2 px-1.5 py-0.5 text-xs text-muted"
        >global</span>
        <span class="flex-1" />
        <button
          type="button"
          class="text-xs text-muted hover:text-fg"
          @click="startEdit(p)"
        >
          edit
        </button>
        <button
          type="button"
          class="text-xs text-muted hover:text-bad"
          @click="remove(p)"
        >
          delete
        </button>
      </li>
    </ul>
    <p
      v-else-if="editing === null"
      class="mb-3 text-sm text-muted"
    >
      No providers configured yet.
    </p>

    <!-- Add / edit form -->
    <form
      v-if="editing !== null"
      class="grid gap-3 rounded border border-border p-3 sm:grid-cols-2"
      data-test="provider-form"
      @submit.prevent="save"
    >
      <label class="flex flex-col gap-1 text-sm">
        <span class="font-medium">Name</span>
        <input
          v-model="form.name"
          required
          class="rounded border border-border bg-surface px-2 py-1"
          data-test="provider-name"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="font-medium">Wire</span>
        <select
          v-model="form.wire"
          class="rounded border border-border bg-surface px-2 py-1"
          data-test="provider-wire"
        >
          <option
            v-for="w in WIRES"
            :key="w"
            :value="w"
          >
            {{ w }}
          </option>
        </select>
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="font-medium">Model</span>
        <input
          v-model="form.model"
          required
          placeholder="claude-sonnet-4-6"
          class="rounded border border-border bg-surface px-2 py-1"
          data-test="provider-model"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="font-medium">Base URL <span class="text-muted">(optional)</span></span>
        <input
          v-model="form.base_url"
          class="rounded border border-border bg-surface px-2 py-1"
          data-test="provider-base-url"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm sm:col-span-2">
        <span class="font-medium">API key</span>
        <input
          v-model="form.api_key"
          type="password"
          autocomplete="off"
          :placeholder="editing ? 'leave blank to keep the stored key' : ''"
          class="rounded border border-border bg-surface px-2 py-1"
          data-test="provider-key"
        >
      </label>
      <label class="flex items-center gap-2 text-sm">
        <input
          v-model="form.is_default"
          type="checkbox"
          data-test="provider-default"
        >
        Default provider
      </label>
      <label
        v-if="isAdmin"
        class="flex items-center gap-2 text-sm"
      >
        <input
          v-model="form.is_global"
          type="checkbox"
          data-test="provider-global"
        >
        Global (all users)
      </label>
      <div class="flex gap-2 sm:col-span-2">
        <button
          type="submit"
          class="rounded bg-fg px-3 py-1 text-sm font-medium text-surface hover:opacity-90"
          data-test="provider-save"
        >
          Save
        </button>
        <button
          type="button"
          class="rounded border border-border px-3 py-1 text-sm hover:bg-surface-2"
          @click="editing = null"
        >
          Cancel
        </button>
      </div>
    </form>
  </section>
</template>
