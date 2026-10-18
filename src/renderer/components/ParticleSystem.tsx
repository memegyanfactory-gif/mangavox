import { useEffect, useRef } from 'react'
import { getThemePreset, type ParticleStyle } from '../../shared/appearance'
import { useAppearanceStore } from '../stores/appearance.store'

interface Particle {
  x: number
  y: number
  vx: number
  vy: number
  size: number
  length: number
  alpha: number
  angle: number
  orbit: number
  color: string
}

function createParticle(style: ParticleStyle, colors: string[], width: number, height: number): Particle {
  const falling = ['line-rain', 'data-rain', 'streaks', 'petals', 'sparse-lines'].includes(style)
  const rising = style === 'bubbles'
  return {
    x: Math.random() * width,
    y: falling ? -80 + Math.random() * (height + 80) : Math.random() * height,
    vx: style === 'streaks' ? 0.8 + Math.random() * 1.6 : -0.18 + Math.random() * 0.36,
    vy: rising ? -(0.12 + Math.random() * 0.35) : 0.14 + Math.random() * (style === 'data-rain' ? 1.1 : 0.7),
    size: 0.8 + Math.random() * (style === 'bubbles' ? 5 : style === 'dust' ? 1.8 : 2.8),
    length: 18 + Math.random() * (style === 'line-rain' ? 90 : 42),
    alpha: 0.05 + Math.random() * 0.16,
    angle: Math.random() * Math.PI * 2,
    orbit: 30 + Math.random() * Math.min(width, height) * 0.35,
    color: colors[Math.floor(Math.random() * colors.length)],
  }
}

export function ParticleSystem({ focusMode = false }: { focusMode?: boolean }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const presetId = useAppearanceStore((state) => state.preset)
  const intensity = useAppearanceStore((state) => state.particleIntensity)
  const reduceMotion = useAppearanceStore((state) => state.reduceMotion)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || intensity === 'off') return
    const ctx = canvas.getContext('2d')
    if (!ctx) return

    const preset = getThemePreset(presetId)
    const style = preset.particleStyle
    let particles: Particle[] = []
    let frame = 0
    let raf = 0
    let hidden = document.hidden
    const systemReduced = window.matchMedia('(prefers-reduced-motion: reduce)')
    const motionScale = reduceMotion || systemReduced.matches ? 0.22 : 1
    const density = intensity === 'high' ? 1.65 : intensity === 'medium' ? 1 : 0.58

    const resize = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2)
      const width = window.innerWidth
      const height = window.innerHeight
      canvas.width = Math.floor(width * dpr)
      canvas.height = Math.floor(height * dpr)
      canvas.style.width = `${width}px`
      canvas.style.height = `${height}px`
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      const base = Math.floor((width * height) / 19000)
      const count = Math.min(150, Math.max(28, Math.floor(base * density)))
      particles = Array.from({ length: count }, () => createParticle(style, preset.colors.particle, width, height))
    }

    const reset = (particle: Particle, width: number, height: number) => {
      Object.assign(particle, createParticle(style, preset.colors.particle, width, height))
      particle.y = style === 'bubbles' ? height + 20 : -particle.length - 20
    }

    const drawLine = (p: Particle, diagonal = false) => {
      ctx.globalAlpha = p.alpha
      ctx.strokeStyle = p.color
      ctx.lineWidth = Math.max(0.5, p.size * 0.45)
      ctx.lineCap = 'round'
      ctx.beginPath()
      ctx.moveTo(p.x - (diagonal ? p.length * 0.45 : 0), p.y - p.length)
      ctx.lineTo(p.x, p.y)
      ctx.stroke()
    }

    const drawDot = (p: Particle, radius = p.size) => {
      ctx.globalAlpha = p.alpha
      ctx.fillStyle = p.color
      ctx.beginPath()
      ctx.arc(p.x, p.y, radius, 0, Math.PI * 2)
      ctx.fill()
    }

    const animate = () => {
      if (hidden) return
      const width = window.innerWidth
      const height = window.innerHeight
      frame += 0.0035 * motionScale
      ctx.clearRect(0, 0, width, height)
      ctx.save()
      ctx.globalCompositeOperation = 'screen'

      particles.forEach((p, index) => {
        if (style === 'linked-dots') {
          p.x += p.vx * motionScale
          p.y += p.vy * motionScale
          if (p.x < -10 || p.x > width + 10) p.vx *= -1
          if (p.y < -10 || p.y > height + 10) p.vy *= -1
          drawDot(p, 1.2 + p.size * 0.35)
          for (let j = index + 1; j < particles.length; j++) {
            const other = particles[j]
            const dx = p.x - other.x
            const dy = p.y - other.y
            const distance = Math.hypot(dx, dy)
            if (distance < 120) {
              ctx.globalAlpha = (1 - distance / 120) * 0.055
              ctx.strokeStyle = p.color
              ctx.lineWidth = 0.6
              ctx.beginPath()
              ctx.moveTo(p.x, p.y)
              ctx.lineTo(other.x, other.y)
              ctx.stroke()
            }
          }
        } else if (style === 'orbits' || style === 'circular-flow') {
          p.angle += (0.0007 + p.vy * 0.001) * motionScale
          const cx = width * (style === 'orbits' ? 0.72 : 0.5)
          const cy = height * (style === 'orbits' ? 0.32 : 0.52)
          const squash = style === 'orbits' ? 0.46 : 0.72
          p.x = cx + Math.cos(p.angle + frame) * p.orbit
          p.y = cy + Math.sin(p.angle + frame) * p.orbit * squash
          drawDot(p, 0.7 + p.size * 0.35)
        } else if (style === 'dust') {
          p.angle += 0.003 * motionScale
          p.x += (p.vx + Math.cos(p.angle) * 0.08) * motionScale
          p.y -= (0.05 + p.vy * 0.16) * motionScale
          if (p.y < -10 || p.x < -10 || p.x > width + 10) reset(p, width, height)
          drawDot(p, p.size * 0.55)
        } else if (style === 'bubbles') {
          p.angle += 0.008 * motionScale
          p.x += Math.sin(p.angle) * 0.16 * motionScale
          p.y += p.vy * motionScale
          if (p.y < -20) reset(p, width, height)
          ctx.globalAlpha = p.alpha * 0.8
          ctx.strokeStyle = p.color
          ctx.lineWidth = 0.7
          ctx.beginPath()
          ctx.arc(p.x, p.y, p.size, 0, Math.PI * 2)
          ctx.stroke()
        } else if (style === 'petals') {
          p.angle += 0.016 * motionScale
          p.x += (p.vx + Math.sin(p.angle) * 0.45) * motionScale
          p.y += p.vy * motionScale
          if (p.y > height + 20) reset(p, width, height)
          ctx.save()
          ctx.translate(p.x, p.y)
          ctx.rotate(p.angle)
          ctx.globalAlpha = p.alpha
          ctx.fillStyle = p.color
          ctx.beginPath()
          ctx.ellipse(0, 0, p.size * 1.5, p.size * 0.65, 0, 0, Math.PI * 2)
          ctx.fill()
          ctx.restore()
        } else {
          p.x += p.vx * motionScale
          p.y += p.vy * motionScale
          if (p.y - p.length > height + 30 || p.x > width + 60) reset(p, width, height)
          drawLine(p, style === 'streaks')
        }
      })

      ctx.restore()
      raf = requestAnimationFrame(animate)
    }

    const onVisibility = () => {
      hidden = document.hidden
      if (!hidden) {
        cancelAnimationFrame(raf)
        raf = requestAnimationFrame(animate)
      }
    }

    resize()
    window.addEventListener('resize', resize)
    document.addEventListener('visibilitychange', onVisibility)
    raf = requestAnimationFrame(animate)

    return () => {
      cancelAnimationFrame(raf)
      window.removeEventListener('resize', resize)
      document.removeEventListener('visibilitychange', onVisibility)
      ctx.clearRect(0, 0, window.innerWidth, window.innerHeight)
    }
  }, [presetId, intensity, reduceMotion])

  if (intensity === 'off') return null
  return (
    <canvas
      ref={canvasRef}
      className="fixed inset-0 pointer-events-none transition-opacity duration-500"
      style={{ zIndex: 0, opacity: focusMode ? 0.12 : intensity === 'high' ? 0.76 : intensity === 'medium' ? 0.62 : 0.4 }}
      aria-hidden
    />
  )
}
