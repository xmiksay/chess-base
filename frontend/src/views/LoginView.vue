<script setup lang="ts">
import { ref, computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '../stores/auth'

const auth = useAuthStore()
const route = useRoute()
const router = useRouter()

// Toggle between signing in and creating an account.
const mode = ref('login')
const username = ref('')
const password = ref('')
const submitting = ref(false)

const isRegister = computed(() => mode.value === 'register')
const heading = computed(() => (isRegister.value ? 'Create account' : 'Sign in'))

function toggleMode() {
  mode.value = isRegister.value ? 'login' : 'register'
  auth.error = null
}

async function submit() {
  if (submitting.value) return
  submitting.value = true
  const ok = isRegister.value
    ? await auth.register(username.value, password.value)
    : await auth.login(username.value, password.value)
  submitting.value = false
  if (ok) {
    const target = typeof route.query.redirect === 'string' ? route.query.redirect : '/'
    router.replace(target)
  }
}

async function signOut() {
  await auth.logout()
}
</script>

<template>
  <div class="mx-auto max-w-sm p-6">
    <!-- Local mode has no login: the single user is the implicit admin. -->
    <template v-if="auth.ready && !auth.isServerMode">
      <h2 class="text-lg font-semibold">
        Sign in
      </h2>
      <p class="mt-2 text-sm text-muted">
        You're running in local mode — signed in as the local admin. No login required.
      </p>
    </template>

    <!-- Already authenticated in server mode: offer to sign out. -->
    <template v-else-if="auth.user">
      <h2 class="text-lg font-semibold">
        Signed in
      </h2>
      <p class="mt-2 text-sm text-muted">
        Signed in as <span class="font-medium text-fg">{{ auth.user.id }}</span>.
      </p>
      <button
        type="button"
        class="mt-4 rounded bg-fg px-4 py-2 text-sm font-medium text-surface hover:opacity-90"
        @click="signOut"
      >
        Sign out
      </button>
    </template>

    <!-- Server mode, not signed in: the register / login form. -->
    <template v-else>
      <h2 class="text-lg font-semibold">
        {{ heading }}
      </h2>
      <form
        class="mt-4 space-y-4"
        @submit.prevent="submit"
      >
        <div>
          <label
            for="username"
            class="block text-sm font-medium text-fg"
          >Username</label>
          <input
            id="username"
            v-model="username"
            name="username"
            type="text"
            autocomplete="username"
            required
            class="mt-1 w-full rounded border border-border px-3 py-2 text-sm"
          >
        </div>
        <div>
          <label
            for="password"
            class="block text-sm font-medium text-fg"
          >Password</label>
          <input
            id="password"
            v-model="password"
            name="password"
            type="password"
            :autocomplete="isRegister ? 'new-password' : 'current-password'"
            required
            class="mt-1 w-full rounded border border-border px-3 py-2 text-sm"
          >
        </div>

        <p
          v-if="auth.error"
          class="text-sm text-bad"
          role="alert"
        >
          {{ auth.error }}
        </p>

        <button
          type="submit"
          :disabled="submitting"
          class="w-full rounded bg-fg px-4 py-2 text-sm font-medium text-surface hover:opacity-90 disabled:opacity-50"
        >
          {{ isRegister ? 'Create account' : 'Sign in' }}
        </button>
      </form>

      <p class="mt-4 text-sm text-muted">
        {{ isRegister ? 'Already have an account?' : "Don't have an account?" }}
        <button
          type="button"
          class="font-medium text-fg hover:underline"
          @click="toggleMode"
        >
          {{ isRegister ? 'Sign in' : 'Create one' }}
        </button>
      </p>
    </template>
  </div>
</template>
