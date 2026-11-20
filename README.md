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
- **Estrellas de GitHub** — arriba a la derecha de la cabecera, tras el contador de túneles, se
  muestra `★ 42` con las estrellas de [fir3desert/WgcTUI](https://github.com/fir3desert/WgcTUI)
  (`★ …` mientras carga, `★ —` si no se pudo obtener, `★ 1.2k` a partir de mil). Se consultan
  una vez por hora en segundo plano con el `curl` del sistema, sin bloquear la interfaz, y se
  guardan en caché en `/var/cache/wgctui/stars.txt`. Es la **única petición de red** del
  programa. El menú de Esc no oscurece esa esquina. Se ocultan en `OPTIONS → General →
  GitHub stars` (o con `show_stars = off`): entonces no se muestra nada ni se hace ninguna
  petición. Si la cabecera es muy estrecha, las estrellas son lo primero que se quita.
- **Opciones (OPTIONS)** — pantalla como la de runetui: pestañas arriba, los ajustes de la
  pestaña a la izquierda y la descripción del seleccionado a la derecha. `↑/↓` mueven,
  `←/→` (o `Tab`) cambian de pestaña, `Enter` o `Espacio` cambian el ajuste y `Esc` vuelve.

  **General**

  | Ajuste            | Qué hace                                                                 |
  |-------------------|--------------------------------------------------------------------------|
  | `GitHub stars`    | Muestra (ACTIVADA) u oculta (DESACTIVADA) el contador de estrellas de la cabecera. Al ocultarlo wgctui no hace ninguna petición de red. |
  | `Update interval` | Frecuencia de actualización, como en btop: cada cuánto se leen los datos de WireGuard (estado, tráfico y gráfica). Por defecto **1000 ms**, de **100** a **20000**. `Enter` y luego `+` / `-` (o `+` / `-` directamente): de 100 en 100 ms hasta 1000 y de 500 en 500 a partir de ahí. `Enter` o `Esc` terminan el ajuste. |

  **Animaciones** — tres interruptores independientes (ACTIVADA / DESACTIVADA):

  | Interruptor        | Qué anima                                                         | Si se desactiva                                       |
  |--------------------|-------------------------------------------------------------------|-------------------------------------------------------|
  | `Header animation` | El texto de la esquina superior izquierda (barrido y onda).       | Vuelve la etiqueta ` wgctui ` (fondo rojo), sin animar. |
  | `Banner animation` | El banner grande del menú de Esc (barrido de luz y onda).         | El banner se ve ya terminado y estático.              |
  | `Menu animation`   | La aparición del propio menú de Esc (el fondo se oscurece y el menú entra suavemente). | El menú aparece de golpe, sin transición. |

  Cada una funciona por su cuenta: desactivar una no afecta a las otras. Los spinners de
  «TRABAJANDO…» no dependen de estos ajustes.

## Ajustes

Los ajustes se guardan en `/etc/wgctui.conf` (el programa siempre acaba corriendo como root),
en cuanto los cambias. Por defecto está todo activado y la frecuencia es de 1000 ms:

```ini
# wgctui
update_ms = 1000
show_stars = on
header_animation = on
banner_animation = on
menu_animation = on
```

`show_stars` y las animaciones son `on` / `off` (también valen `true`/`false`, `yes`/`no`, `1`/`0`).
`update_ms` va de 100 a 20000; un valor fuera de rango se ajusta al límite más cercano. El
archivo se lee con tolerancia: líneas rotas, claves desconocidas o valores que no se entienden
se ignoran y lo que falte queda en su valor por defecto. La clave antigua
`animations = on|off` sigue funcionando y fija las tres animaciones a la vez (las claves nuevas
que vengan después la sobrescriben).

## Rendimiento

El bucle de dibujado solo va rápido (~60 fps) mientras haya una animación activa **y visible**:
el barrido inicial de la cabecera, la animación del banner con el menú grande abierto o la
aparición del menú. La onda infinita de la cabecera se dibuja a ~30 fps. Con las tres
animaciones desactivadas vuelve a la cadencia de reposo (100 ms).

La lectura de datos de WireGuard sigue `update_ms`. Con 100 ms no se apilan lecturas: si una
tarda, no se pide otra hasta que llegue (o pasen 5 s).

## Desarrollo

```sh
cargo build --release   # binario en target/release/wgctui
cargo test              # config, teclas, cadencia del bucle y pantallas (TestBackend)
# Volcar las pantallas a texto para revisarlas:
cargo test dump_screens -- --ignored --nocapture
```
