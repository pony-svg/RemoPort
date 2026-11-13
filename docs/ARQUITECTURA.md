# Arquitectura de Remoport

Estado medido en el commit `8b9769c` (2026-11-05) y arquitectura objetivo para
alta cohesión y bajo acoplamiento. El plan para llegar está en
[PLAN_DE_IMPLEMENTACION.md](PLAN_DE_IMPLEMENTACION.md).

Este documento separa tres cosas y no las mezcla:

- **Medido**: salió de un comando sobre el código (líneas, campos, dependencias).
- **Leído**: se deduce de leer el código fuente, propio o de una dependencia. No
  se reprodujo en ejecución.
- **Propuesto**: no existe todavía.

## 1. Qué hay hoy (medido)

Dos crates en un workspace, 14 352 líneas de Rust, 120 tests (50 de la app,
65 del motor, 5 de integración).

| Crate | Qué es | Líneas | Depende de |
|---|---|---|---|
| `remoport` (binario) | Interfaz egui, sesión, plataforma | 7 834 | `remoport-rdp`, egui/eframe, cpal, wayland, keyring |
| `remoport-rdp` (librería) | Motor RDP sobre IronRDP | 6 239 + 279 de integración | IronRDP, tokio |

### El binario `remoport`

| Archivo | Líneas | Tests | Qué hace hoy |
|---|---|---|---|
| `src/session.rs` | 2 546 | 24 | Todo lo de una sesión: formulario de acceso, llavero, ciclo de vida, reconexión, eventos del motor, tamaño y escala, teclado, ratón, portapapeles, dibujo |
| `src/app.rs` | 1 558 | 0 | Ventana principal, navegación y las cuatro páginas |
| `src/dialogs.rs` | 746 | 0 | Diálogos de PC y de espacio de trabajo |
| `src/audio.rs` | 630 | 6 | Altavoz y micrófono con cpal |
| `src/i18n.rs` | 606 | 4 | Tabla de textos en español |
| `src/model.rs` | 485 | 8 | Tipos del dominio, rutas, guardado, y etiquetas de interfaz |
| `src/key_grab.rs` | 425 | 2 | Teclado propio en Wayland |
| `src/keymap.rs` | 202 | 6 | Tecla de egui a scancode |
| `main`, `icons`, `theme`, `credentials`, `logging`, `fonts` | 636 | 0 | Arranque, iconos, tema, llavero, registro, fuentes |

### El motor `remoport-rdp`

| Archivo | Líneas | Tests | Qué hace |
|---|---|---|---|
| `drive.rs` | 1 689 | 10 | Carpetas compartidas |
| `session.rs` | 795 | 7 | Bucle de la sesión activa |
| `connect.rs` | 633 | 4 | Conexión, identidad, acceso, registro de canales |
| `microphone.rs` | 506 | 6 | Micrófono |
| `identity.rs` | 453 | 7 | Identidad del PC y almacén de confianza |
| `lib.rs` | 413 | 0 | API pública: `ConnectConfig`, `Command`, `Event`, `Session` |
| `clipboard.rs` | 363 | 5 | Portapapeles de texto |
| `kerberos.rs`, `keyboard_layout.rs`, `sound.rs`, `gateway.rs`, `gfx.rs`, `error.rs`, `net.rs` | 1 387 | 26 | Kerberos, distribución de teclado, sonido, puerta de enlace, gráficos, errores, red |

### Quién depende de quién hoy

```
main ──► app ──► dialogs ──► session   (dialogs usa session::screen_scale)
          │         │
          │         └──► model ──► theme, i18n, logging
          └──► session ──► model, credentials, audio, key_grab, keymap,
                           theme, fonts, i18n, logging, remoport-rdp
                key_grab ──► keymap, logging, remoport-rdp (Command, Commander)
                audio ─────► logging, remoport-rdp (SoundOutput, SoundInput)
```

Dentro del binario nada impide que un módulo importe a otro: todo vive en un
solo crate y el compilador no revisa ninguna frontera.

## 2. Qué está bien y se conserva

- **La frontera con el motor es buena.** La dependencia va en un solo sentido,
  de la app al motor. El motor no arrastra egui, eframe, cpal, wayland ni
  keyring (medido con `cargo tree -p remoport-rdp`).
- **La API del motor es de mensajes.** La app envía `Command` y recibe `Event`.
  No comparte memoria con el hilo de red.
- **El sonido ya usa inversión de dependencias.** El motor define `SoundOutput`
  y `SoundInput`; la app los implementa con cpal. El motor no sabe qué es cpal.
- **Los canales del motor son cohesivos.** `clipboard`, `sound`, `microphone`,
  `drive` y `gfx` solo dependen de la raíz del crate.
- **Los secretos tienen tipo.** Contraseñas y portapapeles viajan en
  `Zeroizing` y sus `Debug` no los imprimen.
- **Sin `unwrap()` ni `expect()` fuera de tests** en los dos crates (medido), y
  un solo bloque `unsafe`, en `src/key_grab.rs:196`.
- **Lo que no funciona está marcado.** Impresoras, tarjetas inteligentes,
  cámaras, WebAuthn y "Reproducir en el PC remoto" aparecen deshabilitados con
  el texto "Aún no disponible". Los espacios de trabajo dicen que no están.

## 3. Problemas de cohesión y acoplamiento

Ordenados por cuánto estorban al cambiar el código. Cada uno trae su evidencia.

### P1. `ActiveSession` reúne seis responsabilidades (cohesión baja)

`src/session.rs` tiene una sola estructura con **44 campos** y 2 130 líneas
antes de los tests. Los campos se agrupan en seis responsabilidades que no se
necesitan entre sí:

| Responsabilidad | Campos | Ejemplos |
|---|---|---|
| Acceso y llavero | 12 | `username`, `password`, `remember`, `keyring`, `session_password` |
| Tamaño y escala | 11 | `canvas_px`, `sent_scale`, `pending_size`, `requested_layout` |
| Ciclo de vida | 6 | `phase`, `session`, `resume`, `bookmark` |
| Teclado y ratón | 6 | `modifiers`, `keys_down`, `key_grab`, `last_pointer` |
| Dibujo | 6 | `screen`, `pointer`, `bar_rect`, `fullscreen_hint_until` |
| Portapapeles y micrófono | 3 | `remote_copy`, `microphone_live` |

Evidencia de que duele:

- `remote_screen` mide 275 líneas y en una sola función mide la ventana, decide
  barras, pinta, dibuja el puntero, traduce el ratón, traduce el teclado y
  maneja el portapapeles.
- Los tests de la regla de tamaño tienen que construir la sesión entera. Por
  ejemplo `a_new_scale_alone_is_asked_of_the_pc` crea un `ActiveSession` con
  llavero, teclado y fases solo para probar una espera de 400 ms.
- Los tres últimos cambios (escala, opciones de pantalla, barras) cayeron todos
  en este archivo.

### P2. El teclado tiene dos caminos y pierde teclas (acoplamiento a egui)

Hay dos fuentes de teclas que no se conocen:

1. El hilo de la interfaz recibe `egui::Event::Key` y lo traduce con
   `keymap::scancode(egui::Key)`. Lleva su propio conjunto de teclas pulsadas,
   `keys_down`.
2. El hilo de `key_grab` recibe `wl_keyboard.key` y envía `Command::Key` al
   motor directamente con un `Commander`. Lleva otro conjunto, `held`.

`egui::Key` no distingue el teclado numérico ni tiene AltGr. Leído en
`egui-winit 0.28.1` y `winit 0.29.15`:

| Tecla | Qué recibe la app | Qué llega al PC |
|---|---|---|
| Numérico `*` | Ningún evento: egui no tiene esa tecla | Nada |
| Numérico `+` | `Key::Plus` | Scancode `0x0D`, que en distribución latinoamericana escribe `¿` |
| Numérico `-` | `Key::Minus` | Scancode `0x0C`, que en latinoamericana escribe `'` |
| Numérico `/` | `Key::Slash` | Scancode `0x35`, que en latinoamericana escribe `-` |
| Numérico `.` | `Key::Period` por el texto, sin posición física | El punto del teclado principal, no la tecla decimal |
| Numérico `0`–`9`, Intro | Las mismas teclas que el teclado principal | Los dígitos de la fila superior |
| AltGr | Ningún evento ni modificador | Nada: `sync_modifiers` solo envía Shift, Ctrl y Alt izquierdos |

Esto explica la falla reportada del teclado numérico. Dos precisiones:

- **No reproduje la falla.** Para el punto, el código dice que sí se envía un
  punto, el del teclado principal. Windows recibe entonces otra tecla que la
  decimal, y un programa como Excel trata distinto la tecla decimal (escribe el
  separador regional). Es una hipótesis, no una medición.
- **AltGr no está probado.** Si el código se comporta como se lee, `@` con
  AltGr+Q no se puede escribir dentro del PC. Hay que probarlo.

La causa es de diseño: la app toma las teclas de una abstracción que ya perdió
la posición física. El listener de Wayland sí recibe el código físico de todas
las teclas y hoy solo se usa para cuatro.

### P3. Los datos del PC están copiados campo por campo

`AddEditPcDialog` repite 23 de los 26 campos de `DeviceBookmark` y los copia a
mano al abrir y al guardar. Añadir una opción obliga a tocar el modelo, la
estructura del diálogo, su `Default`, `open_edit` y el guardado. El commit
`500a251` (escala) lo muestra: seis fragmentos de `dialogs.rs` y diez archivos
en total para una sola opción.

Esa copia es también la razón por la que cuatro opciones estuvieron guardándose
sin que ninguna sesión las leyera hasta `b3e80a8`.

### P4. `ConnectConfig` se construye literal en cinco sitios

Es una estructura plana de 21 campos públicos sin constructor. La construyen
`src/session.rs` y cuatro tests del motor (`connect.rs`, `kerberos.rs`,
`gateway.rs`, `tests/identity_flow.rs`). Cada campo nuevo rompe los cinco: los
commits `500a251` y `b3e80a8` tocaron tres archivos del motor solo para añadir
una línea a un test.

### P5. `model.rs` mezcla dominio, disco y textos

El mismo archivo define los tipos, calcula rutas, lee y escribe `state.json`, y
produce etiquetas traducidas (`display_label` llama a `t()`). Además depende de
`theme::Appearance`: el modelo depende de la interfaz, al revés de lo deseable.

### P6. `connect()` del motor conoce todos los canales

Una función de 220 líneas abre el transporte, registra portapapeles, sonido,
micrófono, carpetas, gráficos y control de pantalla, negocia TLS, comprueba la
identidad y firma. `connect.rs` importa diez crates de IronRDP. Añadir RAIL
para RemoteApp significa alargar esa función.

`Event::Progress(String)` es en la práctica un canal de registro: la app lo
único que hace con él es escribirlo en el log (`src/session.rs:908`).

### P7. `app.rs` son cuatro páginas en un tipo

`RemoportApp` contiene la barra lateral, la cabecera, Favoritos, Dispositivos,
Aplicaciones y Configuración. Es grande pero cambia poco; es el problema menos
urgente.

### P8. Ninguna frontera se comprueba

No hay documento de arquitectura ni comprobación en CI. Las reglas de hoy
existen por costumbre.

## 4. Arquitectura objetivo (propuesta)

Tres capas. Las dependencias apuntan hacia abajo y nunca hacia arriba.

```
┌───────────────────────────────────────────────────────────────┐
│ remoport  (binario)                                           │
│   ui/        interfaz egui: ventana, páginas, diálogos,       │
│              vista de la sesión, tema, textos                 │
│   platform/  adaptadores: sonido (cpal), teclado (Wayland),   │
│              llavero                                          │
│   main.rs    une los adaptadores con el núcleo                │
└──────────────┬────────────────────────────────────────────────┘
               ▼
┌───────────────────────────────────────────────────────────────┐
│ remoport-core  (librería nueva, sin interfaz ni plataforma)   │
│   domain     el PC guardado y sus opciones                    │
│   store      leer y escribir el estado, rutas, permisos       │
│   session/   controller  ciclo de vida y reconexión           │
│              display     tamaño, escala y colocación          │
│              input       teclas por código físico, ratón      │
│              signin      acceso y contraseñas en memoria      │
│   ports      SecretStore, fuente de teclas                    │
│   logging                                                     │
└──────────────┬────────────────────────────────────────────────┘
               ▼
┌───────────────────────────────────────────────────────────────┐
│ remoport-rdp  (motor, ya existe)                              │
│   api        ConnectConfig, Command, Event, Session           │
│   connect/   transporte, identidad, acceso                    │
│   channels/  clipboard, sound, microphone, drive, gfx,        │
│              display  (y rail cuando llegue RemoteApp)        │
│   session/   bucle, comandos, salida                          │
└───────────────────────────────────────────────────────────────┘
```

### Por qué un crate nuevo y no solo carpetas

En Rust el compilador solo impide una dependencia entre crates. Dentro de un
crate cualquier módulo puede importar cualquier otro. Con `remoport-core` como
crate sin egui en su `Cargo.toml`, que la lógica de sesión use la interfaz deja
de ser una regla que hay que recordar y pasa a ser un error de compilación.

El coste es mover archivos y ajustar visibilidad. El workspace ya existe.

### Reglas de dependencia

Cada regla tiene un comando que la comprueba. Irán a
`scripts/check-boundaries.sh` y a CI a medida que cada fase las haga ciertas.

| # | Regla | Cómo se comprueba | ¿Se cumple hoy? |
|---|---|---|---|
| R1 | El motor no depende de interfaz ni plataforma | `cargo tree -p remoport-rdp` sin egui, eframe, cpal, wayland-client, keyring, winit | Sí |
| R2 | El núcleo no depende de interfaz ni plataforma | Igual sobre `remoport-core` | No existe |
| R3 | `ui/` no nombra cpal, wayland, keyring ni nix; `platform/` no nombra egui | `grep` sobre las carpetas | No hay carpetas |
| R4 | Solo `core::session::controller` arranca una `remoport_rdp::Session` | `grep 'Session::start'` | Sí, pero en la vista |
| R5 | Un solo dueño de las teclas pulsadas; `platform/` no envía `Command` | `grep 'Command' src/platform` vacío | No |
| R6 | El núcleo no produce texto para el usuario: devuelve tipos y la interfaz traduce | `grep 'i18n' remoport-core` vacío | No |
| R7 | Ningún secreto llega al log ni a un `Debug` | Tests existentes y revisión | Sí |

### Geometría sin interfaz

La regla de tamaño usa `Rect`, `Vec2` y `Pos2`. Vienen de `emath`, el crate de
matemáticas de egui, que no trae interfaz. `remoport-core` puede depender de
`emath` y mantener esos tipos sin depender de egui. Ya está en `Cargo.lock`
(0.28.1).

### El teclado en la arquitectura objetivo

```
platform/wayland_keys ──(canal: tecla física, bloqueos, foco)──► hilo de interfaz
                                                                    │
egui (solo si no hay Wayland) ─────────────────────────────────────►│
                                                                    ▼
                                              core::session::input
                                              · tabla código físico → scancode
                                              · único conjunto de teclas pulsadas
                                              · atajos propios (Ctrl+Alt+Intro)
                                              · regla del pegado
                                                                    ▼
                                                        Command::Key al motor
```

`platform/` deja de conocer `Command`. Una sola pieza, comprobable con tests sin
ventana, decide qué se envía. El teclado numérico, AltGr y la tecla Windows
salen de la misma tabla.

### Dónde caerá lo que viene

| Función | Lugar |
|---|---|
| Canal RAIL (RemoteApp) | `remoport-rdp/channels/rail`, con sus `Command` y `Event` |
| Ventanas de RemoteApp | `ui/`, reutilizando `core::session::controller` con otra vista |
| Fuente RD Web (espacios de trabajo) | Crate propio sin interfaz ni motor: procesa XML y HTTP de un servidor, y conviene poder probarlo aislado |
| Entra ID | Obtención del token en el núcleo; el motor solo recibe el token |

La separación controlador/vista es lo que hace barato RemoteApp: una ventana de
aplicación remota es el mismo controlador con otra forma de dibujar.

## 5. Lo que no se propone, y por qué

- **Renombrar `DeviceBookmark`.** Es el nombre del formato guardado. Cambiarlo
  no mejora cohesión y arriesga los datos existentes.
- **Un puerto para el registro.** `logging::event` es global y simple. Un trait
  para el log sería una abstracción con una sola implementación.
- **Partir `drive.rs` ahora.** Es grande (1 222 líneas sin tests) pero hace una
  sola cosa. Se parte cuando haya que volver a tocarlo.
- **Actualizar egui.** No se evaluó. Afecta a teclado y a varias ventanas; es
  una decisión aparte.
- **Reescribir.** Todo el plan es mover y extraer con la app funcionando en cada
  paso.

## 6. Límites de este análisis

- Las estructuras y dependencias se midieron sobre todo el código. La lectura a
  fondo en esta pasada fue de `session.rs`, `keymap.rs`, `key_grab.rs`,
  `lib.rs` del motor, `connect.rs` y las partes citadas. De `app.rs`,
  `drive.rs`, `identity.rs` y `audio.rs` se revisó el esquema, no cada línea.
- La tabla del teclado numérico y AltGr es lectura de código, no una prueba.
- Los tests no cubren el dibujo. Cualquier cambio en la vista solo se puede
  confirmar a mano contra un PC real.
- La mejora es estructural. Salvo el teclado, ninguna fase cambia lo que ve
  quien usa la aplicación.
