<script setup>
import { Sun, Moon } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { useTheme } from '@/lib/core/theme'

const { isDark, toggle } = useTheme()
</script>

<template>
  <Button variant="ghost" size="icon" aria-label="Toggle theme" @click="toggle">
    <Sun v-if="isDark" class="size-4" />
    <Moon v-else class="size-4" />
  </Button>
</template>
