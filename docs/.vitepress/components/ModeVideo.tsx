import { defineComponent } from 'vue'
import { useData, withBase } from 'vitepress'

export default defineComponent({
  name: 'ModeVideo',
  props: {
    file: { type: String, required: true },
    title: { type: String, required: true },
    description: { type: String, required: true },
  },
  setup(props) {
    const { lang } = useData()
    return () => {
      const source = withBase(`/video/${props.file}`)
      return (
        <figure class="mode-video">
          <video
            class="mode-video-player"
            controls
            playsinline
            preload="metadata"
            aria-label={props.title}
          >
            <source src={source} type="video/mp4" />
            {lang.value === 'en-US' ? 'Your browser cannot play this MP4. ' : '当前浏览器无法播放 MP4。'}
            <a href={source}>{lang.value === 'en-US' ? 'Open the video directly' : '直接打开视频'}</a>
          </video>
          <figcaption>
            <strong>{props.title}</strong>
            <span>{props.description}</span>
          </figcaption>
        </figure>
      )
    }
  },
})
