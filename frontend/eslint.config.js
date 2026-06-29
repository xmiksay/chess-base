import js from '@eslint/js'
import pluginVue from 'eslint-plugin-vue'
import tseslint from 'typescript-eslint'

// Flat config: typescript-eslint parses .ts and (via vue-eslint-parser) the
// `<script lang="ts">` blocks in .vue SFCs. `no-undef` is delegated to the type
// checker (vue-tsc), which knows the DOM/Vitest globals from tsconfig `lib`/`types`.
export default tseslint.config(
  { ignores: ['dist/**', 'node_modules/**'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...pluginVue.configs['flat/recommended'],
  {
    files: ['**/*.vue'],
    languageOptions: {
      parserOptions: { parser: tseslint.parser },
    },
  },
  {
    rules: {
      'vue/multi-word-component-names': 'off',
      'no-undef': 'off',
    },
  },
)
