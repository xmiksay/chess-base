import js from '@eslint/js'
import pluginVue from 'eslint-plugin-vue'

export default [
  js.configs.recommended,
  ...pluginVue.configs['flat/recommended'],
  {
    languageOptions: {
      ecmaVersion: 'latest',
      sourceType: 'module',
      globals: { fetch: 'readonly', localStorage: 'readonly', console: 'readonly' },
    },
    rules: {
      'vue/multi-word-component-names': 'off',
    },
  },
  {
    files: ['**/*.test.js'],
    languageOptions: {
      globals: { vi: 'readonly', describe: 'readonly', it: 'readonly', expect: 'readonly' },
    },
  },
  { ignores: ['dist/**', 'node_modules/**'] },
]
