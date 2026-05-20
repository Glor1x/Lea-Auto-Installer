const canvas = document.getElementById('particles')
const ctx = canvas.getContext('2d')

function resizeCanvas() {
	canvas.width = window.innerWidth
	canvas.height = window.innerHeight
}
resizeCanvas()
window.addEventListener('resize', resizeCanvas)

const particles = Array.from({ length: 35 }, () => ({
	x: Math.random() * canvas.width,
	y: Math.random() * canvas.height,
	r: Math.random() * 1.2 + 0.3,
	dx: (Math.random() - 0.5) * 0.25,
	dy: (Math.random() - 0.5) * 0.25,
	o: Math.random() * 0.35 + 0.08,
}))

function drawParticles() {
	ctx.clearRect(0, 0, canvas.width, canvas.height)
	particles.forEach(p => {
		ctx.beginPath()
		ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2)
		ctx.fillStyle = `rgba(61, 220, 110, ${p.o})`
		ctx.fill()
		p.x += p.dx
		p.y += p.dy
		if (p.x < 0 || p.x > canvas.width) p.dx *= -1
		if (p.y < 0 || p.y > canvas.height) p.dy *= -1
	})
	requestAnimationFrame(drawParticles)
}
drawParticles()

async function waitForTauri() {
	return new Promise(resolve => {
		if (window.__TAURI__) {
			resolve()
			return
		}
		const t = setInterval(() => {
			if (window.__TAURI__) {
				clearInterval(t)
				resolve()
			}
		}, 50)
	})
}

const tauriInvoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args)
const tauriListen = (ev, cb) => window.__TAURI__.event.listen(ev, cb)

const CIRCUMFERENCE = 439.82
const points = []
let displayPercent = 0
let targetPercent = 0
let rafId = null
let doneFired = false
let installationConfirmed = false

function tickProgress() {
	const diff = targetPercent - displayPercent

	if (Math.abs(diff) < 0.05) {
		displayPercent = targetPercent
		rafId = null
	} else {
		const speed = Math.min(Math.max(Math.abs(diff) * 0.03, 0.15), 0.8)
		displayPercent += speed
		rafId = requestAnimationFrame(tickProgress)
	}

	const clamped = Math.min(displayPercent, 100)
	const offset = CIRCUMFERENCE - (clamped / 100) * CIRCUMFERENCE

	const fillEl = document.getElementById('circle-fill')
	const glowEl = document.getElementById('circle-glow')
	const pctEl = document.getElementById('progress-percent')
	const statEl = document.getElementById('progress-status')

	if (fillEl) fillEl.style.strokeDashoffset = offset
	if (glowEl) glowEl.style.strokeDashoffset = offset
	if (pctEl) pctEl.textContent = Math.round(clamped) + '%'

	if (statEl) {
		for (let i = points.length - 1; i >= 0; i--) {
			if (displayPercent >= points[i].percent) {
				statEl.textContent = points[i].status
				break
			}
		}
	}

	if (displayPercent >= 100 && installationConfirmed && !doneFired) {
		doneFired = true
		setTimeout(() => showScreen('screen-done'), 400)
	}
}

function setProgress(percent, status) {
	points.push({ percent, status })
	const nextTarget = percent >= 100 && !installationConfirmed ? 99 : percent
	if (nextTarget > targetPercent) targetPercent = nextTarget
	displayPercent = targetPercent
	if (rafId) {
		cancelAnimationFrame(rafId)
		rafId = null
	}
	tickProgress()
}

function resetProgress() {
	points.length = 0
	displayPercent = 0
	targetPercent = 0
	doneFired = false
	installationConfirmed = false
	if (rafId) {
		cancelAnimationFrame(rafId)
		rafId = null
	}
	setProgress(0, 'Подготовка...')
}

function confirmInstallationComplete(status) {
	installationConfirmed = true
	setProgress(100, status || 'LEA успешно установлен!')
}

function failInstallation(status) {
	installationConfirmed = false
	doneFired = true
	targetPercent = Math.min(targetPercent, 99)
	const statEl = document.getElementById('progress-status')
	if (statEl) statEl.textContent = status
}

function showScreen(id) {
	document.querySelectorAll('.screen').forEach(s => s.classList.add('hidden'))
	document.getElementById(id).classList.remove('hidden')
}

class LEAInstaller {
	constructor() {
		this.gamePath = ''
		this.pathValid = false
	}

	async init() {
		await waitForTauri()
		this.bindEvents()
		await this.setupProgressListener()
	}

	bindEvents() {
		document
			.getElementById('browse-btn')
			.addEventListener('click', () => this.selectFolder())

		let inputTimer = null
		document.getElementById('game-path').addEventListener('input', () => {
			clearTimeout(inputTimer)
			inputTimer = setTimeout(() => this.validateManualPath(), 600)
		})

		document
			.getElementById('agree-terms')
			.addEventListener('change', () => this.updateInstallButton())

		document
			.getElementById('install-btn')
			.addEventListener('click', () => this.tryInstall())

		document
			.getElementById('kill-game-btn')
			.addEventListener('click', () => this.killGameAndInstall())

		document
			.getElementById('cancel-install-btn')
			.addEventListener('click', () => showScreen('screen-main'))

		document
			.getElementById('minimize-btn')
			.addEventListener('click', () => tauriInvoke('minimize_app'))

		document
			.getElementById('close-btn')
			.addEventListener('click', () => tauriInvoke('close_app'))

		document
			.getElementById('close-done-btn')
			.addEventListener('click', () => tauriInvoke('close_app'))

		document.querySelectorAll('a[href]').forEach(link => {
			link.addEventListener('click', e => {
				e.preventDefault()
				window.__TAURI__.shell.open(link.href)
			})
		})
	}

	async setupProgressListener() {
		await tauriListen('install-progress', event => {
			const { percent, status } = event.payload
			setProgress(percent, status)
		})
	}

	async selectFolder() {
		try {
			const result = await tauriInvoke('select_folder')
			if (result) {
				this.gamePath = result.path
				this.pathValid = result.is_valid
				document.getElementById('game-path').value = result.path
				this.updateValidation(result.is_valid)
				this.updateInstallButton()
			}
		} catch (err) {
			console.error('Ошибка выбора папки:', err)
		}
	}

	async validateManualPath() {
		const input = document.getElementById('game-path').value.trim()
		if (!input) {
			this.gamePath = ''
			this.pathValid = false
			this.resetValidation()
			this.updateInstallButton()
			return
		}

		try {
			const result = await tauriInvoke('validate_path', { path: input })
			this.gamePath = result.path
			this.pathValid = result.is_valid
			this.updateValidation(result.is_valid)
			this.updateInstallButton()
		} catch (err) {
			console.error('Ошибка валидации:', err)
		}
	}

	resetValidation() {
		const v = document.getElementById('validation')
		v.className = 'validation'
		v.innerHTML =
			'<i class="fas fa-info-circle"></i><span>Выберите корневую папку с gta_sa.exe</span>'
	}

	updateValidation(isValid) {
		const v = document.getElementById('validation')
		if (isValid) {
			v.className = 'validation valid'
			v.innerHTML =
				'<i class="fas fa-check-circle"></i><span>Папка игры найдена</span>'
		} else {
			v.className = 'validation invalid'
			v.innerHTML =
				'<i class="fas fa-exclamation-triangle"></i><span>Файл gta_sa.exe не найден</span>'
		}
	}

	updateInstallButton() {
		const agreed = document.getElementById('agree-terms').checked
		document.getElementById('install-btn').disabled = !(
			agreed && this.pathValid
		)
	}

	async tryInstall() {
		if (!this.gamePath || !this.pathValid) return

		try {
			const running = await tauriInvoke('check_game_running')
			if (running) {
				showScreen('screen-game-running')
			} else {
				this.startInstallation()
			}
		} catch (err) {
			this.startInstallation()
		}
	}

	async killGameAndInstall() {
		try {
			await tauriInvoke('kill_game')
			await new Promise(r => setTimeout(r, 1000))
		} catch (err) {
			console.error('Ошибка закрытия игры:', err)
		}
		this.startInstallation()
	}

	async startInstallation() {
		resetProgress()
		showScreen('screen-install')

		try {
			const result = await tauriInvoke('start_installation', {
				gamePath: this.gamePath,
			})
			if (result.success) {
				confirmInstallationComplete('LEA успешно установлен!')
			} else {
				failInstallation('Ошибка: ' + result.error)
			}
		} catch (err) {
			failInstallation('Ошибка: ' + err)
		}
	}
}

document.addEventListener('DOMContentLoaded', async () => {
	const installer = new LEAInstaller()
	await installer.init()
	await waitForTauri()
	window.__TAURI__.event.emit('page-ready', {})
})
