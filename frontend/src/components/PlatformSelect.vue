<template>
  <div
    ref="rootEl"
    class="platform-select"
    :class="[`platform-select--${sizeClass}` , { 'platform-select--open': isOpen }]"
    tabindex="0"
    @click="handleRootClick"
    @keydown.enter.prevent="toggleDropdown"
    @keydown.space.prevent="toggleDropdown"
    @keydown.escape.stop="closeDropdown"
  >
    <div class="platform-select__current">
      <component
        :is="currentOption.icon"
        class="platform-select__icon"
      />
      <div class="platform-select__text">
        <p class="platform-select__label">
          {{ currentOption.label }}
        </p>
        <p class="platform-select__hint">
          {{ currentOption.hint }}
        </p>
      </div>
    </div>
    <ChevronDown class="platform-select__chevron" />

    <transition name="platform-select-fade">
      <ul
        v-if="isOpen"
        class="platform-select__dropdown"
      >
        <li
          v-for="option in displayOptions"
          :key="option.value"
          class="platform-select__option"
          :class="{ 'platform-select__option--active': option.value === modelValue }"
          @click.stop="selectOption(option.value)"
        >
          <component
            :is="option.icon"
            class="platform-select__option-icon"
          />
          <div class="platform-select__option-copy">
            <p class="platform-select__option-label">
              {{ option.label }}
            </p>
            <p class="platform-select__option-hint">
              {{ option.hint }}
            </p>
          </div>
          <Check
            v-if="option.value === modelValue"
            class="platform-select__option-check"
          />
        </li>
      </ul>
    </transition>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { Check, ChevronDown } from 'lucide-vue-next'
import { defaultPlatformOptions, type PlatformOption } from '@/config/platform-presets'
import { useI18n } from '@/i18n'

const props = defineProps<{
  modelValue: string
  size?: 'md' | 'lg'
  options?: PlatformOption[]
}>()

const emit = defineEmits<{
  (event: 'update:modelValue', value: string): void
}>()

const { t } = useI18n()
const rootEl = ref<HTMLElement | null>(null)
const isOpen = ref(false)
const sizeClass = computed(() => props.size ?? 'md')

const resolvedOptions = computed(() => props.options ?? defaultPlatformOptions)
const displayOptions = computed(() => resolvedOptions.value.map(option => ({
  ...option,
  label: t(option.labelKey),
  hint: t(option.hintKey),
})))

const currentOption = computed(() =>
  displayOptions.value.find((option) => option.value === props.modelValue) ?? displayOptions.value[0]
)

function toggleDropdown() {
  isOpen.value = !isOpen.value
}

function closeDropdown() {
  isOpen.value = false
}

function selectOption(value: string) {
  if (value !== props.modelValue) {
    emit('update:modelValue', value)
  }
  closeDropdown()
}

function handleRootClick(event: MouseEvent) {
  const dropdown = rootEl.value?.querySelector('.platform-select__dropdown')
  if (dropdown?.contains(event.target as Node)) {
    return
  }
  toggleDropdown()
}

function handleClickOutside(event: MouseEvent) {
  if (!rootEl.value) {
    return
  }
  if (!rootEl.value.contains(event.target as Node)) {
    closeDropdown()
  }
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside)
})

onBeforeUnmount(() => {
  document.removeEventListener('click', handleClickOutside)
})
</script>

<style scoped>
.platform-select {
  position: relative;
  width: 11rem;
  border: 1px solid var(--color-border);
  border-radius: 0.9rem;
  background-color: var(--color-background);
  padding: 0.55rem 0.85rem;
  cursor: pointer;
  transition: border-color 0.2s ease, box-shadow 0.2s ease, background-color 0.2s ease;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.75rem;
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.12);
}

.dark .platform-select {
  background-color: var(--color-background);
}

.platform-select--lg {
  width: 13rem;
}

.platform-select:focus-visible,
.platform-select--open {
  border-color: var(--color-primary);
  box-shadow: 0 0 0 3px rgba(204, 120, 92, 0.2);
}

.platform-select__current {
  display: flex;
  align-items: center;
  gap: 0.65rem;
}

.platform-select__icon {
  width: 1.1rem;
  height: 1.1rem;
  color: var(--color-primary);
}

.platform-select__text {
  display: flex;
  flex-direction: column;
  line-height: 1.1;
}

.platform-select__label {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--color-text);
  white-space: nowrap;
}

.platform-select__hint {
  font-size: 0.7rem;
  color: #91918d;
  white-space: nowrap;
}

.dark .platform-select__hint {
  color: #a8a29e;
}

.platform-select__chevron {
  width: 0.9rem;
  height: 0.9rem;
  color: var(--color-border-soft);
}

.platform-select__dropdown {
  position: absolute;
  top: calc(100% + 0.45rem);
  left: 0;
  right: 0;
  padding: 0.35rem;
  border-radius: 1rem;
  border: 1px solid var(--color-border);
  background-color: var(--color-background);
  box-shadow: 0 25px 55px rgba(0, 0, 0, 0.25);
  z-index: 30;
  backdrop-filter: blur(16px);
}

.platform-select__option {
  display: flex;
  align-items: center;
  gap: 0.65rem;
  padding: 0.55rem 0.6rem;
  border-radius: 0.75rem;
  transition: background 0.2s ease, color 0.2s ease;
}

.platform-select__option:hover {
  background: rgba(204, 120, 92, 0.1);
}

.platform-select__option--active {
  background: rgba(204, 120, 92, 0.18);
}

.platform-select__option-icon {
  width: 1rem;
  height: 1rem;
  color: var(--color-primary);
}

.platform-select__option-copy {
  display: flex;
  flex-direction: column;
  line-height: 1.1;
}

.platform-select__option-label {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--color-text);
  white-space: nowrap;
}

.platform-select__option-hint {
  font-size: 0.7rem;
  color: #91918d;
  white-space: nowrap;
}

.dark .platform-select__option-hint {
  color: #a8a29e;
}

.platform-select__option-check {
  margin-left: auto;
  width: 0.85rem;
  height: 0.85rem;
  color: var(--color-primary);
}

.platform-select-fade-enter-active,
.platform-select-fade-leave-active {
  transition: opacity 0.15s ease, transform 0.15s ease;
}

.platform-select-fade-enter-from,
.platform-select-fade-leave-to {
  opacity: 0;
  transform: translateY(-6px);
}
</style>
