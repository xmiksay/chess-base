<script setup lang="ts">
// Shared LLM provider/model picker (issue #214): a grouped <select> (optgroup
// per provider row) of the caller's selectable models. The effective default
// (lib/providers effectiveDefault) is preselected and labeled "(default)";
// while it is selected the v-model is `null` — callers then omit both request
// fields and the server resolves the default itself. Kept generic so the
// assistant picker can reuse it.
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { effectiveDefault, modelOptions } from '../lib/providers'
import type { ModelChoice } from '../lib/providers'
import type { ProviderInfo } from '../types'

interface Props {
  modelValue: ModelChoice | null
  /** Pre-fetched provider rows to avoid a double fetch; omitted ⇒ fetch on mount. */
  providers?: ProviderInfo[] | null
}
const props = withDefaults(defineProps<Props>(), { providers: null })
const emit = defineEmits<{ 'update:modelValue': [value: ModelChoice | null] }>()

const fetched = ref<ProviderInfo[]>([])
const rows = computed(() => props.providers ?? fetched.value)
const groups = computed(() => modelOptions(rows.value))
const fallback = computed(() => effectiveDefault(rows.value))

// Option values encode (provider, model) with a separator no name contains.
const SEP = '\u001f'
const key = (provider: string, model: string) => `${provider}${SEP}${model}`
const defaultKey = computed(() =>
  fallback.value ? key(fallback.value.name, fallback.value.model) : '',
)
const selected = computed(() =>
  props.modelValue
    ? key(props.modelValue.provider, props.modelValue.model)
    : defaultKey.value,
)

function onChange(e: Event) {
  const value = (e.target as HTMLSelectElement).value
  if (!value || value === defaultKey.value) {
    emit('update:modelValue', null)
    return
  }
  const [provider, model] = value.split(SEP)
  emit('update:modelValue', { provider, model })
}

onMounted(async () => {
  if (props.providers) return
  try {
    fetched.value = await api.providers.list()
  } catch {
    fetched.value = [] // no rows ⇒ the disabled placeholder renders
  }
})
</script>

<template>
  <select
    data-test="model-select"
    class="rounded border border-border bg-surface px-2 py-1 text-fg"
    :value="selected"
    @change="onChange"
  >
    <option
      v-if="!groups.length"
      value=""
      disabled
    >
      No LLM providers configured
    </option>
    <optgroup
      v-for="g in groups"
      :key="g.provider"
      :label="g.provider"
    >
      <option
        v-for="m in g.models"
        :key="key(g.provider, m)"
        :value="key(g.provider, m)"
      >
        {{ m }}{{ key(g.provider, m) === defaultKey ? ' (default)' : '' }}
      </option>
    </optgroup>
  </select>
</template>
