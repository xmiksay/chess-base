<script setup lang="ts">
// Admin user list + password reset (ADR-0055). A reset signs the target out
// everywhere; the admin hands the new password over out-of-band. Your own row
// has no reset — use the Account section, which asks for the current password.
import { onMounted, reactive, ref } from 'vue'
import { api } from '../api'
import type { UserSummary } from '../types'

const props = defineProps<{ selfId: string }>()

const users = ref<UserSummary[]>([])
const error = ref<string | null>(null)
const notice = ref<string | null>(null)
// The user whose reset form is open, or null.
const resetting = ref<string | null>(null)
const form = reactive({ next: '', confirm: '' })

async function load() {
  try {
    users.value = await api.users.list()
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  }
}

function startReset(u: UserSummary) {
  Object.assign(form, { next: '', confirm: '' })
  resetting.value = u.id
  error.value = null
  notice.value = null
}

async function reset(u: UserSummary) {
  error.value = null
  if (form.next !== form.confirm) {
    error.value = 'Passwords do not match.'
    return
  }
  try {
    await api.users.resetPassword(u.id, form.next)
    resetting.value = null
    notice.value = `Password for ${u.username} reset; they were signed out.`
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  }
}

onMounted(load)
</script>

<template>
  <section class="rounded border border-border p-4">
    <h2 class="mb-3 text-lg font-semibold">
      Users
    </h2>
    <p
      v-if="error"
      class="mb-2 text-sm text-bad"
      data-test="users-error"
    >
      {{ error }}
    </p>
    <p
      v-if="notice"
      class="mb-2 text-sm text-good"
      data-test="users-notice"
    >
      {{ notice }}
    </p>
    <ul class="divide-y divide-border">
      <li
        v-for="u in users"
        :key="u.id"
        class="py-2 text-sm"
        data-test="user-row"
      >
        <div class="flex items-center gap-2">
          <span class="font-medium">{{ u.username }}</span>
          <span
            v-if="u.is_admin"
            class="rounded bg-surface-2 px-1.5 py-0.5 text-xs text-muted"
          >admin</span>
          <span class="text-xs text-muted">since {{ u.created_at.slice(0, 10) }}</span>
          <span class="flex-1" />
          <button
            v-if="u.id !== props.selfId"
            type="button"
            class="text-xs text-muted hover:text-fg"
            data-test="user-reset"
            @click="startReset(u)"
          >
            reset password
          </button>
        </div>
        <form
          v-if="resetting === u.id"
          class="mt-2 flex flex-wrap items-end gap-2"
          data-test="reset-form"
          @submit.prevent="reset(u)"
        >
          <input
            v-model="form.next"
            type="password"
            required
            minlength="8"
            autocomplete="new-password"
            placeholder="New password"
            class="rounded border border-border bg-surface px-2 py-1"
            data-test="reset-new"
          >
          <input
            v-model="form.confirm"
            type="password"
            required
            minlength="8"
            autocomplete="new-password"
            placeholder="Confirm"
            class="rounded border border-border bg-surface px-2 py-1"
            data-test="reset-confirm"
          >
          <button
            type="submit"
            class="rounded bg-fg px-3 py-1 text-sm font-medium text-surface hover:opacity-90"
            data-test="reset-save"
          >
            Reset
          </button>
          <button
            type="button"
            class="rounded border border-border px-3 py-1 text-sm hover:bg-surface-2"
            @click="resetting = null"
          >
            Cancel
          </button>
        </form>
      </li>
    </ul>
  </section>
</template>
