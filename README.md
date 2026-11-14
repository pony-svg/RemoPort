# Remoport

**Your remote computers and apps, from Linux.**

Remoport is a desktop client for reaching Windows PCs, remote desktops and the apps
your organization publishes, with a clean connection center: favorites, PCs and apps
side by side, and workspaces that bring in what your organization offers.

> **Status: early preview.** Remote desktops open inside Remoport through its own
> RDP engine, [`remoport-rdp`](remoport-rdp), built on IronRDP with parts from
> [RustConn](https://github.com/totoshko88/RustConn) by Anton Isaiev. Screen, keyboard
> and mouse work, the desktop follows the window's size, and the PC's sound and a
> shared text clipboard can be turned on per PC, and a dropped connection comes back
> to the same session; the microphone and local folders can be offered per PC too.
> PCs behind an RD Gateway are reached through it, and accounts that may not use NTLM
> sign in with Kerberos. A session can span several screens, one window each, and go
> full screen (Ctrl+Alt+Enter), where the Windows key and Alt+Tab go to the PC on
> Wayland; a Keys menu sends them from anywhere. The interface is in English or
> Spanish, by the system's language. This still awaits testing against real servers.

## Security

- **The PC's identity is confirmed before your password is used.** A certificate from
  an authority this computer trusts is accepted; any other is shown to you with its
  SHA-256 fingerprint, and nothing is sent until you trust it. A certificate that
  changed since you trusted it is flagged as a possible interception.
- **An RD Gateway must prove who it is**: its certificate has to come from an authority
  this computer trusts and match its name, or Remoport refuses it before sending your
  credentials. The PC behind it is still checked as above.
- **Network Level Authentication is required**, unless you allow a specific PC to
  sign in without it. The original RDP security is never used.
- **Passwords are remembered only if you ask, and only in the system keyring**
  (KWallet or GNOME Keyring). While a session lasts its password stays in memory,
  so a dropped connection can sign in again; it is wiped when the session ends.
- **Nothing is shared with a PC unless you turn it on for that PC.** The clipboard,
  when shared, carries text only: the PC gets just the text you paste into the
  session, and what you copy there reaches this computer only while you are using
  the session. Sound from the PC is played here as uncompressed audio only.
- **The microphone** runs only while the PC asks for it; Remoport shows when it is on
  and lets you mute it.
- **A shared folder** is the only thing the PC can reach on this computer: paths that
  lead outside it, symbolic links included, are refused, and it is read-only unless
  you allow changes. No printer, camera or smart card is offered.
- Remoport's files (your PCs, trusted certificates, activity log) are readable only by
  you, and the log never contains passwords.

## Build and run

```bash
cargo build --release
./target/release/remoport
```

To install it for your user, with its launcher in the app menu and its icon in the
taskbar (`--uninstall` removes it again):

```bash
scripts/install-local.sh
```

## License

Remoport is free software under the **GNU General Public License, version 3 or later**
(see [`LICENSE`](LICENSE)). The Remoport name and logo are not covered by that license;
see [`TRADEMARKS.md`](TRADEMARKS.md).

Remoport is an independent project. It is not affiliated with, endorsed by or
sponsored by Microsoft. Windows is a trademark of the Microsoft group of companies.

---

## En español

**Tus equipos y apps remotas, desde Linux.**

Remoport es un cliente de escritorio para conectarte a PCs con Windows, escritorios
remotos y las apps que publica tu organización, con un centro de conexiones claro:
favoritos, PCs y apps juntos, y espacios de trabajo que traen lo que tu organización
te ofrece.

> **Estado: vista previa.** Los escritorios remotos se abren dentro de Remoport con su
> propio motor RDP, `remoport-rdp`, construido sobre IronRDP con partes de
> [RustConn](https://github.com/totoshko88/RustConn), de Anton Isaiev. Pantalla, teclado
> y ratón funcionan, el escritorio se ajusta al tamaño de la ventana, y el sonido del
> PC y un portapapeles de texto compartido se activan por PC, una conexión caída
> vuelve a la misma sesión, y el micrófono y carpetas locales también se ofrecen por
> PC, a los PCs detrás de un RD Gateway se llega a través de él, y las cuentas que no
> pueden usar NTLM inician sesión con Kerberos. Una sesión puede usar varias
> pantallas, una ventana cada una, y pasar a pantalla completa (Ctrl+Alt+Enter), donde
> la tecla Windows y Alt+Tab van al PC en Wayland; un menú Teclas las envía desde
> cualquier parte. La interfaz está en inglés o español, según el idioma del
> sistema. Falta probarlo contra servidores reales.

**Seguridad:** Remoport confirma la identidad del PC antes de usar tu contraseña (y te
muestra la huella del certificado si no puede verificarlo), rechaza un RD Gateway cuyo
certificado no venga de una autoridad de confianza antes de enviarle nada, exige autenticación a nivel
de red (NLA) salvo que lo permitas para un PC concreto, guarda contraseñas solo si lo
pides y solo en el llavero del sistema (mientras dura una sesión la contraseña queda en
memoria para reconectar, y se borra al terminar), no comparte nada con un PC salvo lo que actives
para ese PC (el portapapeles solo lleva texto: el PC recibe únicamente lo que pegas en
la sesión; el micrófono solo funciona mientras el PC lo pide y lo puedes silenciar; una
carpeta compartida es lo único que el PC alcanza y es de solo lectura salvo que permitas
cambios), y sus archivos solo los puedes leer tú.

Remoport es software libre bajo la **GNU GPL, versión 3 o posterior**. El nombre y el
logo de Remoport no están cubiertos por esa licencia: consulta
[`TRADEMARKS.md`](TRADEMARKS.md). Remoport es un proyecto independiente, sin relación
con Microsoft.
