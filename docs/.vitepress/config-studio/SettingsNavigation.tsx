import { computed, defineComponent } from 'vue'
import { categories, pages } from './navigation'

export default defineComponent({
  props: { page: { type: String, required: true }, mobile: Boolean, disabled: Boolean },
  emits: { select: (_page: string) => true },
  setup(props, { emit }) {
    const current = computed(() => pages.find(page => page.id === props.page)!)
    return () => props.mobile ? (
      <div inert={props.disabled} class="ks-mobile-navigation">
        <select aria-label="设置分类" value={current.value.category} onChange={event => emit('select', pages.find(page => page.category === (event.target as HTMLSelectElement).value)!.id)}>
          {categories.map(category => <option value={category.id}>{category.label}</option>)}
        </select>
        <select aria-label="配置功能" value={props.page} onChange={event => emit('select', (event.target as HTMLSelectElement).value)}>
          {pages.filter(page => page.category === current.value.category).map(page => <option value={page.id}>{page.label}</option>)}
        </select>
      </div>
    ) : (
      <nav inert={props.disabled} class="ks-settings-nav" aria-label="配置分类">
        {categories.map(category => <section key={category.id}>
          <h2>{category.label}</h2>
          {pages.filter(page => page.category === category.id).map(page => <button key={page.id} aria-current={props.page === page.id ? 'page' : undefined} onClick={() => emit('select', page.id)}>
            {page.label}{page.mode && <small>{page.mode}</small>}
          </button>)}
        </section>)}
      </nav>
    )
  },
})
