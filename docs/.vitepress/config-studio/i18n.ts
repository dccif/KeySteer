import { inject, provide, ref, watch, type InjectionKey, type Ref } from 'vue'
import { translate, type StudioLocale } from './messages.ts'

const localeKey: InjectionKey<Ref<StudioLocale>> = Symbol('studio-locale')

/** Instance-local state: switching language never remounts the editor or changes TOML. */
export function provideStudioLocale(siteLanguage: Ref<string>) {
  const locale = ref<StudioLocale>(siteLanguage.value.startsWith('en') ? 'en' : 'zh')
  watch(siteLanguage, value => { locale.value = value.startsWith('en') ? 'en' : 'zh' })
  provide(localeKey, locale)
  return locale
}

export function useStudioI18n() {
  const locale = inject(localeKey, ref<StudioLocale>('zh'))
  return { locale, t: (text: string, values: unknown[] = []) => translate(text, locale.value, values) }
}
