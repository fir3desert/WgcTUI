# WgTUI-Client
Wireguard TUI to manage client connections

## Novedades

- **Buscador de túneles** — `/` o `Ctrl+P` abre un cuadro de búsqueda difusa igual que el
  "Find file" de runetui: escribes unas letras del nombre, los resultados se ordenan y las
  letras que coinciden se resaltan. `↑/↓` mueven, `Enter` selecciona el túnel y `Esc` cierra.
- **Cabecera de una fila** — arriba a la izquierda, el texto `WGC TUI` en una sola línea, con
  un barrido de luz inicial y después una onda infinita en el degradado rojo de la marca.
  La cabecera mide siempre 1 fila, sea cual sea el tamaño de la terminal.
- **Menú de Esc** — `Esc` abre un menú con banner animado y solo tres entradas: `OPTIONS`
  (seleccionada al abrir), `HELP` y `QUIT`. `Esc` dentro del menú lo cierra. El menú grande
  necesita 26 filas; en terminales más pequeñas se usa la versión compacta.
- **Opciones (OPTIONS)** — tres interruptores independientes, cada uno con su descripción y
  su estado (ACTIVADA / DESACTIVADA). `↑/↓` se mueven entre ellos; `Enter`, `Espacio` o `←/→`
  cambian el seleccionado:

  | Interruptor        | Qué anima                                                         | Si se desactiva                                       |
  |--------------------|-------------------------------------------------------------------|-------------------------------------------------------|
  | `Header animation` | El texto de la esquina superior izquierda (barrido y onda).       | Vuelve la etiqueta ` wgctui ` (fondo rojo), sin animar. |
  | `Banner animation` | El banner grande del menú de Esc (barrido de luz y onda).         | El banner se ve ya terminado y estático.              |
  | `Menu animation`   | La aparición del propio menú de Esc (el fondo se oscurece y el menú entra suavemente). | El menú aparece de golpe, sin transición. |

  Cada una funciona por su cuenta: desactivar una no afecta a las otras. Los spinners de
  «TRABAJANDO…» no dependen de estos ajustes.

## Ajustes

Los ajustes se guardan en `/etc/wgctui.conf` (el programa siempre acaba corriendo como root).
Por defecto está todo activado:

```ini
# wgctui
header_animation = on
banner_animation = on
menu_animation = on
```

Los valores son `on` / `off` (también valen `true`/`false`, `yes`/`no`, `1`/`0`). El archivo se
lee con tolerancia: líneas rotas, claves desconocidas o valores que no se entienden se ignoran
y lo que falte queda activado. La clave antigua `animations = on|off` sigue funcionando y fija
las tres a la vez (las claves nuevas que vengan después la sobrescriben).

## Rendimiento

El bucle de dibujado solo va rápido (~60 fps) mientras haya una animación activa **y visible**:
el barrido inicial de la cabecera, la animación del banner con el menú grande abierto o la
aparición del menú. La onda infinita de la cabecera se dibuja a ~30 fps. Con las tres
animaciones desactivadas vuelve a la cadencia de reposo (100 ms).

## Desarrollo

```sh
cargo build --release   # binario en target/release/wgctui
cargo test              # config, teclas, cadencia del bucle y pantallas (TestBackend)
# Volcar las pantallas a texto para revisarlas:
cargo test dump_screens -- --ignored --nocapture
```
