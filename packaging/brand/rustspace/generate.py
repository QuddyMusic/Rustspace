# Варианты значка Rustspace: тёмная скруглённая плитка с бликом и ободком, свой глиф.
RUST = "#e8743b"
VIOLET = "#7c6cf6"

def tile(bg_top="#28243d", bg_bottom="#110f1b", extra_defs=""):
    return f'''<defs>
  <linearGradient id="bg" x1="0" y1="0" x2="0" y2="256" gradientUnits="userSpaceOnUse">
    <stop stop-color="{bg_top}"/><stop offset="1" stop-color="{bg_bottom}"/>
  </linearGradient>
  <linearGradient id="sheen" x1="0" y1="0" x2="0" y2="256" gradientUnits="userSpaceOnUse">
    <stop stop-color="#fff" stop-opacity="0.09"/><stop offset="0.42" stop-color="#fff" stop-opacity="0"/>
  </linearGradient>
  <linearGradient id="rim" x1="0" y1="0" x2="0" y2="256" gradientUnits="userSpaceOnUse">
    <stop stop-color="#fff" stop-opacity="0.5"/><stop offset="0.5" stop-color="#fff" stop-opacity="0.15"/><stop offset="1" stop-color="#fff" stop-opacity="0.05"/>
  </linearGradient>
  <filter id="glow" x="-30%" y="-30%" width="160%" height="160%">
    <feGaussianBlur in="SourceAlpha" stdDeviation="8"/>
    <feColorMatrix values="0 0 0 0 0.05  0 0 0 0 0  0 0 0 0 0.14  0 0 0 0.35 0"/>
    <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
  {extra_defs}
</defs>
<rect width="256" height="256" rx="57" fill="url(#bg)"/>
<rect width="256" height="256" rx="57" fill="url(#sheen)"/>
<rect x="0.75" y="0.75" width="254.5" height="254.5" rx="56.3" fill="none" stroke="url(#rim)" stroke-width="1.5"/>
'''

def grad(id_, a, b):
    return f'<linearGradient id="{id_}" x1="70" y1="50" x2="190" y2="215" gradientUnits="userSpaceOnUse"><stop stop-color="{a}"/><stop offset="1" stop-color="{b}"/></linearGradient>'

# Округлая «R» штрихами: ствол, чаша, нога.
R_STEM = "M98 62 V194"
R_BOWL = "M98 62 H138 A37 37 0 0 1 138 136 H98"
R_LEG = "M134 136 L170 194"
S = 'stroke-width="32" stroke-linecap="round" stroke-linejoin="round" fill="none"'

def svg(body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256" fill="none">\n{body}</svg>\n'

variants = {}

# 1. «Оксид»: та же манера градиента, фиолетовый уступает ржавчине.
variants["1-oxide"] = svg(tile(extra_defs=grad("g", "#ffffff", RUST)) +
    f'<g filter="url(#glow)"><path d="{R_STEM} {R_BOWL} M134 136 L170 194" stroke="url(#g)" {S}/></g>\n')

# 2. «Биметалл»: фиолетовая «R», нога — ржавая.
variants["2-bimetal"] = svg(tile(extra_defs=grad("g", "#ffffff", VIOLET)) +
    f'<g filter="url(#glow)"><path d="{R_STEM} {R_BOWL}" stroke="url(#g)" {S}/>'
    f'<path d="{R_LEG}" stroke="{RUST}" {S}/></g>\n')

# 3. «Каретка»: фиолетовая «R» и ржавый курсор терминала.
variants["3-caret"] = svg(tile(extra_defs=grad("g", "#ffffff", VIOLET)) +
    f'<g filter="url(#glow)" transform="translate(-14 0)"><path d="{R_STEM} {R_BOWL} {R_LEG}" stroke="url(#g)" {S}/>'
    f'<rect x="190" y="150" width="26" height="44" rx="6" fill="{RUST}"/></g>\n')

# 4. «.rs»: строчные r и s, под ними ржавая клавиша пробела.
W4 = 'stroke-width="20" stroke-linecap="round" stroke-linejoin="round" fill="none"'
RS = (f'<path d="M78 180 V108 M78 136 C78 114 94 104 116 107" stroke="url(#g)" {W4}/>'
      f'<path d="M184 116 C176 106 162 102 149 104 C130 107 127 130 146 137 L164 143 C186 151 185 176 162 180 C146 183 132 178 126 168" stroke="url(#g)" {W4}/>')
variants["4-rs"] = svg(tile(extra_defs=grad("g", "#ffffff", VIOLET)) +
    f'<g filter="url(#glow)" transform="translate(-3 -8)">{RS}<rect x="68" y="200" width="124" height="18" rx="9" fill="{RUST}"/></g>\n')

# 5. «Патина»: плитка уходит от фиолетового к ржавому, «R» светлая.
variants["5-patina"] = svg(tile("#3a2c55", "#3b1d12",
    extra_defs=grad("g", "#ffffff", "#ffd2b8") + '<linearGradient id="diag" x1="0" y1="0" x2="256" y2="256" gradientUnits="userSpaceOnUse"><stop stop-color="#2c2548"/><stop offset="1" stop-color="#4a2414"/></linearGradient>').replace('fill="url(#bg)"','fill="url(#diag)"') +
    f'<g filter="url(#glow)"><path d="{R_STEM} {R_BOWL} {R_LEG}" stroke="url(#g)" {S}/></g>\n')

# Запуск из этого каталога: python3 generate.py; icon.svg — выбранный вариант.
CHOSEN = "1-oxide"
for name, data in variants.items():
    open(f"variant-{name}.svg", "w").write(data)
open("icon.svg", "w").write(variants[CHOSEN])
print(" ".join(variants))
