<script setup lang="ts">
// Change your own password (ADR-0055). The server keeps this session and signs
// every other one out (plus revokes OAuth/MCP tokens). Server mode only — the
// parent mounts it behind auth.isServerMode.
import { reactive, ref } from 'vue'
import { api } from '../api'

const form = reactive({ current: '', next: '', confirm: '' })
const error = ref<string | null>(null)
const done = ref(false)

async function save() {
  error.value = null
  done.value = false
  if (form.next !== form.confirm) {
    error.value = 'New passwords do not match.'
    return
  }
  try {
    await api.auth.changePassword(form.current, form.next)
    Object.assign(form, { current: '', next: '', confirm: '' })
    done.value = true
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  }
}
</script>

<template>
  <section class="rounded border border-border p-4">
    <h2 class="mb-1 text-lg font-semibold">
      Account
    </h2>
    <p class="mb-3 text-sm text-muted">
      Changing your password signs out your other sessions and disconnects
      MCP clients (they must re-authorize).
    </p>
    <form
      class="grid gap-3 sm:grid-cols-3"
      data-test="password-form"
      @submit.prevent="save"
    >
      <label class="flex flex-col gap-1 text-sm">
        <span class="font-medium">Current password</span>
        <input
          v-model="form.current"
          type="password"
          required
          autocomplete="current-password"
          class="rounded border border-border bg-surface px-2 py-1"
          data-test="password-current"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="font-medium">New password</span>
        <input
          v-model="form.next"
          type="password"
          required
          minlength="8"
          autocomplete="new-password"
          class="rounded border border-border bg-surface px-2 py-1"
          data-test="password-new"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="font-medium">Confirm new password</span>
        <input
          v-model="form.confirm"
          type="password"
          required
          minlength="8"
          autocomplete="new-password"
          class="rounded border border-border bg-surface px-2 py-1"
          data-test="password-confirm"
        >
      </label>
      <div class="flex items-center gap-3 sm:col-span-3">
        <button
          type="submit"
          class="rounded bg-fg px-3 py-1 text-sm font-medium text-surface hover:opacity-90"
          data-test="password-save"
        >
          Change password
        </button>
        <span
          v-if="done"
          class="text-sm text-good"
          data-test="password-done"
        >Password changed.</span>
        <span
          v-if="error"
          class="text-sm text-bad"
          data-test="password-error"
        >{{ error }}</span>
      </div>
    </form>
  </section>
</template>
