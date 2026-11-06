//! The interface in the user's language: English as written in the code,
//! Spanish from the table below when the system asks for it. `REMOPORT_LANG`
//! overrides the system. The activity log and the engine's technical detail
//! stay in English.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

/// Whether the interface is in Spanish. Tests always run in English.
pub fn spanish() -> bool {
    static SPANISH: OnceLock<bool> = OnceLock::new();
    *SPANISH.get_or_init(|| !cfg!(test) && asks_for_spanish(|name| std::env::var(name).ok()))
}

/// Whether the environment's first language setting is Spanish, in the order
/// POSIX gives them.
fn asks_for_spanish(variable: impl Fn(&str) -> Option<String>) -> bool {
    ["REMOPORT_LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| variable(name).filter(|value| !value.is_empty()))
        .is_some_and(|value| {
            let first = value.split(':').next().unwrap_or_default().to_lowercase();
            first == "es" || first.starts_with("es_") || first.starts_with("es-")
        })
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| TABLE.iter().copied().collect())
}

/// `english` in the user's language.
pub fn t(english: &'static str) -> &'static str {
    if spanish() {
        table().get(english).copied().unwrap_or(english)
    } else {
        english
    }
}

/// `template` in the user's language, each `{}` taking the next of `args`.
pub fn tf(template: &'static str, args: &[&dyn Display]) -> String {
    let mut out = String::new();
    let mut parts = t(template).split("{}");
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    let mut args = args.iter();
    for part in parts {
        if let Some(arg) = args.next() {
            out.push_str(&arg.to_string());
        }
        out.push_str(part);
    }
    out
}

/// English and Spanish, by the English text.
const TABLE: &[(&str, &str)] = &[
    (
        "Ctrl+Alt+Enter leaves full screen; the bar shows at the top edge",
        "Ctrl+Alt+Enter sale de la pantalla completa; la barra aparece en el borde superior",
    ),
    (
        "⛶ Leave full screen",
        "⛶ Salir de pantalla completa",
    ),
    (" · {} screens",
     " · {} pantallas"),
    ("+ Add Folder",
     "+ Agregar carpeta"),
    ("About Remoport",
     "Acerca de Remoport"),
    ("Account",
     "Cuenta"),
    ("Add",
     "Agregar"),
    ("Add PC",
     "Agregar PC"),
    ("Add a PC",
     "Agregar un PC"),
    ("Add a PC by its name or address, or subscribe to your organization's workspace.",
     "Agrega un PC por su nombre o dirección, o suscríbete al espacio de trabajo de tu organización."),
    ("Add a PC…",
     "Agregar un PC…"),
    ("Add to favorites",
     "Agregar a favoritos"),
    ("All",
     "Todos"),
    ("Allow changes",
     "Permitir cambios"),
    ("Allow signing in without Network Level Authentication",
     "Permitir iniciar sesión sin autenticación a nivel de red (NLA)"),
    ("Alt+F4 · Close the window",
     "Alt+F4 · Cerrar la ventana"),
    ("Alt+Tab · Switch apps",
     "Alt+Tab · Cambiar de aplicación"),
    ("Appearance",
     "Apariencia"),
    ("Apps",
     "Aplicaciones"),
    ("Apps published by workspaces are not available yet.",
     "Las aplicaciones publicadas por espacios de trabajo aún no están disponibles."),
    ("Audio playback:",
     "Reproducción de audio:"),
    ("Before your password is used, Remoport confirms the PC's identity: a certificate from an authority this computer trusts, or one you checked and trusted yourself.",
     "Antes de usar tu contraseña, Remoport confirma la identidad del PC: un certificado de una autoridad en la que confía este equipo, o uno que revisaste y aceptaste tú."),
    ("Cameras",
     "Cámaras"),
    ("Cancel",
     "Cancelar"),
    ("Cards",
     "Tarjetas"),
    ("Certificate for",
     "Certificado para"),
    ("Certificates you trusted",
     "Certificados que aceptaste"),
    ("Checking the PC's identity before signing in.",
     "Comprobando la identidad del PC antes de iniciar sesión."),
    ("Choose the local folders to share with the remote session:",
     "Elige las carpetas locales que compartirás con la sesión remota:"),
    ("Click the remote screen to type into it",
     "Haz clic en la pantalla remota para escribir en ella"),
    ("Clipboard (text)",
     "Portapapeles (texto)"),
    ("Color quality",
     "Calidad de color"),
    ("Configure remote screen resolution and display options:",
     "Configura la resolución y las opciones de la pantalla remota:"),
    ("Connect",
     "Conectar"),
    ("Connected",
     "Conectado"),
    ("Connecting to {}…",
     "Conectando con {}…"),
    ("Connecting…",
     "Conectando…"),
    ("Ctrl+Alt+Enter switches full screen on and off",
     "Ctrl+Alt+Enter activa y desactiva la pantalla completa"),
    ("Ctrl+Shift+Esc · Task Manager",
     "Ctrl+Shift+Esc · Administrador de tareas"),
    ("Dark",
     "Oscuro"),
    ("Default for this display",
     "La de esta pantalla"),
    ("Delete",
     "Eliminar"),
    ("Delete this PC?",
     "¿Eliminar este PC?"),
    ("Delete…",
     "Eliminar…"),
    ("Devices",
     "Dispositivos"),
    ("Devices & Audio",
     "Dispositivos y audio"),
    ("Disconnected",
     "Desconectado"),
    ("Display",
     "Pantalla"),
    ("Don't connect",
     "No conectar"),
    ("Don't play sound",
     "No reproducir sonido"),
    ("Edit",
     "Editar"),
    ("Edit PC",
     "Editar PC"),
    ("Edit…",
     "Editar…"),
    ("Enter the address of your organization's workspace feed, or your work email:",
     "Escribe la dirección del feed de espacios de trabajo de tu organización, o tu correo de trabajo:"),
    ("Favorites",
     "Favoritos"),
    ("Feedback",
     "Comentarios"),
    ("Fingerprint (SHA-256)",
     "Huella (SHA-256)"),
    ("Fit session to window",
     "Ajustar la sesión a la ventana"),
    ("Folders",
     "Carpetas"),
    ("For accounts that may not use NTLM. Name the PC by its DNS name and sign in as user@example.com. Without a KDC, Remoport looks in krb5.conf, then asks the domain itself.",
     "Para cuentas que no pueden usar NTLM. Nombra el PC por su nombre DNS e inicia sesión como usuario@ejemplo.com. Sin KDC, Remoport lo busca en krb5.conf y luego se lo pide al propio dominio."),
    ("Forget",
     "Olvidar"),
    ("Free software under the GNU GPL, version 3 or later.",
     "Software libre bajo la GNU GPL, versión 3 o posterior."),
    ("Friendly name:",
     "Nombre para mostrar:"),
    ("Gateway account:",
     "Cuenta del gateway:"),
    ("Gateway password",
     "Contraseña del gateway"),
    ("General",
     "General"),
    ("Group",
     "Grupo"),
    ("Group:",
     "Grupo:"),
    ("Help",
     "Ayuda"),
    ("High (32 bit)",
     "Alta (32 bits)"),
    ("Host: {}",
     "Equipo: {}"),
    ("Issued by",
     "Emitido por"),
    ("Its RDP engine is built on IronRDP, with parts from RustConn by Anton Isaiev.",
     "Su motor RDP está construido sobre IronRDP, con partes de RustConn de Anton Isaiev."),
    ("Just once, without saving it:",
     "Solo esta vez, sin guardarlo:"),
    ("Key combinations this computer keeps for itself, or that cannot be typed here",
     "Combinaciones de teclas que este equipo se reserva, o que no se pueden escribir aquí"),
    ("Last connected",
     "Última conexión"),
    ("Leave empty to be asked when connecting",
     "Déjalo vacío para que se pregunte al conectar"),
    ("Leave empty to use the PC's account",
     "Déjalo vacío para usar la cuenta del PC"),
    ("Less secure: the PC shows its sign-in screen before checking who you are. Turn on only for an old PC you trust that cannot use NLA.",
     "Menos seguro: el PC muestra su pantalla de inicio de sesión antes de comprobar quién eres. Actívalo solo para un PC antiguo de confianza que no pueda usar NLA."),
    ("Light",
     "Claro"),
    ("List",
     "Lista"),
    ("Medium (16 bit)",
     "Media (16 bits)"),
    ("Microphone",
     "Micrófono"),
    ("More actions",
     "Más acciones"),
    ("Move this window to its screen, then press Ctrl+Alt+Enter",
     "Mueve esta ventana a su pantalla y pulsa Ctrl+Alt+Enter"),
    ("My administrator confirmed this certificate was replaced",
     "Mi administrador confirmó que este certificado se reemplazó"),
    ("Name",
     "Nombre"),
    ("Name or address",
     "Nombre o dirección"),
    ("Name or address of the PC",
     "Nombre o dirección del PC"),
    ("Network Level Authentication is required, unless you allow a PC to sign in without it in its settings.",
     "Se exige la autenticación a nivel de red (NLA), salvo que en la configuración de un PC permitas iniciar sesión sin ella."),
    ("Never",
     "Nunca"),
    ("New PCs",
     "PCs nuevos"),
    ("No PCs match",
     "Ningún PC coincide"),
    ("No PCs yet",
     "Aún no hay PCs"),
    ("No apps match",
     "Ninguna aplicación coincide"),
    ("No apps yet",
     "Aún no hay aplicaciones"),
    ("No favorites yet",
     "Aún no hay favoritos"),
    ("No folders added yet.",
     "Aún no has agregado carpetas."),
    ("Not available yet",
     "Aún no disponible"),
    ("Nothing to filter by yet",
     "Aún no hay nada por qué filtrar"),
    ("Now presented",
     "El que presenta ahora"),
    ("Open",
     "Abrir"),
    ("Open the ··· menu of a PC or an app and add it to your favorites.",
     "Abre el menú ··· de un PC o una aplicación y agrégalo a tus favoritos."),
    ("Optional",
     "Opcional"),
    ("Optional, e.g. dc01.example.com",
     "Opcional, p. ej. dc01.example.com"),
    ("Optional, e.g. gateway.example.com",
     "Opcional, p. ej. gateway.example.com"),
    ("Order added",
     "Orden de creación"),
    ("PC name cannot be empty",
     "El nombre del PC no puede estar vacío"),
    ("PC name:",
     "Nombre del PC:"),
    ("Password",
     "Contraseña"),
    ("Passwords are remembered only if you ask, and only in the system keyring.",
     "Las contraseñas se recuerdan solo si lo pides, y solo en el llavero del sistema."),
    ("Play on remote PC",
     "Reproducir en el PC remoto"),
    ("Play on this device",
     "Reproducir en este dispositivo"),
    ("Please enter an email or workspace URL",
     "Escribe un correo o la URL de un espacio de trabajo"),
    ("Print Screen",
     "Impr Pant"),
    ("Printers",
     "Impresoras"),
    ("Published by your workspaces",
     "Publicadas por tus espacios de trabajo"),
    ("Reconnecting to {}…",
     "Reconectando con {}…"),
    ("Reconnecting… ({} of {})",
     "Reconectando… ({} de {})"),
    ("Remember in the system keyring",
     "Recordar en el llavero del sistema"),
    ("Remoport cannot confirm this PC's identity",
     "Remoport no puede confirmar la identidad de este PC"),
    ("Remoport confirms the PC's identity before your password is used.",
     "Remoport confirma la identidad del PC antes de usar tu contraseña."),
    ("Remoport could not verify {}: {}. If this is your PC, compare the fingerprint below with the one your administrator gives you before you trust it. Nothing has been sent to it yet.",
     "Remoport no pudo verificar {}: {}. Si es tu PC, compara la huella de abajo con la que te dé tu administrador antes de confiar en él. Aún no se le ha enviado nada."),
    ("Remoport has no folder to keep trusted certificates in (HOME is not set).",
     "Remoport no tiene una carpeta donde guardar los certificados de confianza (HOME no está definido)."),
    ("Remoport opens one window per screen. Move each to its screen and press Ctrl+Alt+Enter to make it fill that screen; the PC follows their sizes.",
     "Remoport abre una ventana por pantalla. Mueve cada una a su pantalla y pulsa Ctrl+Alt+Enter para que la ocupe entera; el PC se ajusta a sus tamaños."),
    ("Remove from favorites",
     "Quitar de favoritos"),
    ("Report a problem",
     "Reportar un problema"),
    ("Report a problem or suggest an idea",
     "Reportar un problema o sugerir una idea"),
    ("Resolution",
     "Resolución"),
    ("Resolution for PCs you add",
     "Resolución de los PCs que agregues"),
    ("Same as this screen ({}%)",
     "Igual que esta pantalla ({}%)"),
    ("Save",
     "Guardar"),
    ("Saved PCs",
     "PCs guardados"),
    ("Saved on this computer",
     "Guardados en este equipo"),
    ("Scale",
     "Escala"),
    ("Screens:",
     "Pantallas:"),
    ("Search",
     "Buscar"),
    ("Security",
     "Seguridad"),
    ("Settings",
     "Configuración"),
    ("Shared with this PC:",
     "Compartido con este PC:"),
    ("Sign in",
     "Iniciar sesión"),
    ("Sign in to an organization…",
     "Iniciar sesión en una organización…"),
    ("Sign in to {}",
     "Iniciar sesión en {}"),
    ("Sign in with Kerberos",
     "Iniciar sesión con Kerberos"),
    ("Smart cards",
     "Tarjetas inteligentes"),
    ("Some programs already open on the PC may not adapt until you reopen them.",
     "Algunos programas ya abiertos en el PC podrían no adaptarse hasta que los vuelvas a abrir."),
    ("Sort by",
     "Ordenar por"),
    ("Source code",
     "Código fuente"),
    ("Stop reconnecting",
     "Dejar de reconectar"),
    ("Subscribe",
     "Suscribirse"),
    ("Subscribe to a workspace",
     "Suscribirse a un espacio de trabajo"),
    ("Subscribe to a workspace…",
     "Suscribirse a un espacio de trabajo…"),
    ("Subscribe to your organization's workspace to open the apps it publishes as windows of their own.",
     "Suscríbete al espacio de trabajo de tu organización para abrir las aplicaciones que publica en ventanas propias."),
    ("The PC can use it only while it asks for it, and Remoport shows when it is on, with a button to mute it.",
     "El PC solo puede usarlo mientras lo pide, y Remoport muestra cuándo está encendido, con un botón para silenciarlo."),
    ("The PC gets only the text you paste into the session, and what you copy there reaches this computer only while you are using the session.",
     "El PC recibe solo el texto que pegas en la sesión, y lo que copias allí llega a este equipo solo mientras usas la sesión."),
    ("The PC gets silence. Click to unmute.",
     "El PC recibe silencio. Haz clic para reactivarlo."),
    ("The PC is using your microphone. Click to mute.",
     "El PC está usando tu micrófono. Haz clic para silenciarlo."),
    ("The PC refused the saved password. Type it again.",
     "El PC rechazó la contraseña guardada. Escríbela de nuevo."),
    ("The PC refused the sign-in. Check the user name and password.",
     "El PC rechazó el inicio de sesión. Revisa el usuario y la contraseña."),
    ("The PC sees each folder as a drive and cannot reach anything outside it. Folders are read-only unless you allow changes.",
     "El PC ve cada carpeta como una unidad y no puede alcanzar nada fuera de ella. Las carpetas son de solo lectura salvo que permitas cambios."),
    ("The PCs and apps you starred",
     "Los PCs y aplicaciones que marcaste"),
    ("The connection dropped. Attempt {} of {}: your session on the PC normally stays open and comes back as you left it.",
     "Se cayó la conexión. Intento {} de {}: tu sesión en el PC normalmente sigue abierta y vuelve tal como la dejaste."),
    ("The connection to {} was lost and could not be brought back after {} attempts ({}).",
     "Se perdió la conexión con {} y no se pudo recuperar tras {} intentos ({})."),
    ("The gateway's certificate must come from an authority this computer trusts. With an account of its own, its password is asked when connecting.",
     "El certificado del gateway debe venir de una autoridad en la que confíe este equipo. Con una cuenta propia, su contraseña se pide al conectar."),
    ("The next connection asks again",
     "La próxima conexión volverá a preguntar"),
    ("The session ended",
     "La sesión terminó"),
    ("The session ended.",
     "La sesión terminó."),
    ("The system keyring is not available, so no password is remembered.",
     "El llavero del sistema no está disponible, así que no se recuerda ninguna contraseña."),
    ("Trust and connect",
     "Confiar y conectar"),
    ("Trust the new certificate",
     "Confiar en el nuevo certificado"),
    ("Try again",
     "Intentar de nuevo"),
    ("Try another name or address, or show all groups.",
     "Prueba otro nombre o dirección, o muestra todos los grupos."),
    ("Try another name, or show all workspaces.",
     "Prueba otro nombre, o muestra todos los espacios de trabajo."),
    ("Use full resolution on HiDPI displays",
     "Usar la resolución completa en pantallas HiDPI"),
    ("User account:",
     "Cuenta de usuario:"),
    ("User name",
     "Nombre de usuario"),
    ("Valid",
     "Válido"),
    ("View",
     "Vista"),
    ("View and sort",
     "Ver y ordenar"),
    ("Waiting for your decision",
     "Esperando tu decisión"),
    ("Warning: this PC's certificate changed",
     "Advertencia: el certificado de este PC cambió"),
    ("WebAuthn / Passkeys",
     "WebAuthn / llaves de acceso"),
    ("Welcome to Remoport",
     "Te damos la bienvenida a Remoport"),
    ("What you do in Remoport is logged to {}",
     "Lo que haces en Remoport se registra en {}"),
    ("Win+D · Show desktop",
     "Win+D · Mostrar el escritorio"),
    ("Win+E · File Explorer",
     "Win+E · Explorador de archivos"),
    ("Win+R · Run",
     "Win+R · Ejecutar"),
    ("Windows key (Start)",
     "Tecla Windows (Inicio)"),
    ("Workspace",
     "Espacio de trabajo"),
    ("Workspace subscriptions are not available yet in this preview.",
     "Las suscripciones a espacios de trabajo aún no están disponibles en esta vista previa."),
    ("You chose not to connect: the PC's identity was not confirmed. Nothing was sent to it.",
     "Elegiste no conectarte: no se confirmó la identidad del PC. No se le envió nada."),
    ("You have not trusted any PC's certificate yourself.",
     "Aún no has aceptado el certificado de ningún PC."),
    ("You stopped reconnecting.",
     "Dejaste de reconectar."),
    ("You trusted (SHA-256)",
     "El que aceptaste (SHA-256)"),
    ("Your PCs and settings are saved in {}",
     "Tus PCs y tu configuración se guardan en {}"),
    ("Your data",
     "Tus datos"),
    ("Your password is remembered in the system keyring.",
     "Tu contraseña queda recordada en el llavero del sistema."),
    ("Your remote computers and apps, from Linux.",
     "Tus equipos y aplicaciones remotos, desde Linux."),
    ("Your remote computers and apps, in one place.",
     "Tus equipos y aplicaciones remotos, en un solo lugar."),
    ("\"{}\" ({}) will be removed from Remoport, with its password if it is remembered in the keyring. The PC itself is not affected.",
     "\"{}\" ({}) se quitará de Remoport, junto con su contraseña si está recordada en el llavero. El PC en sí no se ve afectado."),
    ("\"{}\" would open here",
     "\"{}\" se abriría aquí"),
    ("hostname, IP, or FQDN",
     "nombre, IP o FQDN"),
    ("nowhere (no home folder)",
     "ningún lugar (no hay carpeta personal)"),
    ("or",
     "o"),
    ("standard error only",
     "solo la salida de errores"),
    ("user, DOMAIN\\user or user@domain",
     "usuario, DOMINIO\\usuario o usuario@dominio"),
    ("you@company.com or https://rdweb.example.com/RDWeb/Feed/webfeed.aspx",
     "tu@empresa.com o https://rdweb.example.com/RDWeb/Feed/webfeed.aspx"),
    ("{} does not offer Network Level Authentication, so it would show its sign-in screen before checking who you are. Remoport does not connect that way unless you allow it for this PC (Edit → General), which is less secure.",
     "{} no ofrece autenticación a nivel de red (NLA), así que mostraría su pantalla de inicio de sesión antes de comprobar quién eres. Remoport no se conecta así salvo que lo permitas para este PC (Editar → General), lo cual es menos seguro."),
    ("{} is not a folder on this computer.",
     "{} no es una carpeta de este equipo."),
    ("{} presents a different certificate from the one you trusted. Someone may be intercepting the connection to capture your password. Continue only if your administrator confirms the certificate was replaced.",
     "{} presenta un certificado distinto del que aceptaste. Alguien podría estar interceptando la conexión para capturar tu contraseña. Continúa solo si tu administrador confirma que el certificado se reemplazó."),
    ("{} to {}",
     "{} a {}"),
    ("{} x {} (Custom)",
     "{} x {} (personalizada)"),
    ("{} · screen {}",
     "{} · pantalla {}"),
    ("⌨ Keys",
     "⌨ Teclas"),
    ("Microphone muted",
     "Micrófono silenciado"),
    ("Microphone on",
     "Micrófono encendido"),
    ("⛶ Full screen",
     "⛶ Pantalla completa"),
    ("❌ Close",
     "❌ Cerrar"),
    ("❌ Disconnect",
     "❌ Desconectar"),
    ("🔑 Saved in your keyring",
     "🔑 Guardada en tu llavero"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_language_decides() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string())
            }
        };
        assert!(asks_for_spanish(env(&[("LANG", "es_CO.UTF-8")])));
        assert!(asks_for_spanish(env(&[
            ("LANGUAGE", "es:en"),
            ("LANG", "en_US.UTF-8")
        ])));
        assert!(!asks_for_spanish(env(&[
            ("LANGUAGE", "en:es"),
            ("LANG", "es_CO.UTF-8")
        ])));
        assert!(asks_for_spanish(env(&[
            ("REMOPORT_LANG", "es"),
            ("LANG", "en_US.UTF-8")
        ])));
        assert!(!asks_for_spanish(env(&[("LANG", "et_EE.UTF-8")])));
        assert!(!asks_for_spanish(env(&[])));
    }

    #[test]
    fn values_fill_the_template_in_order() {
        assert_eq!(tf("{} to {}", &[&"a", &1]), "a to 1");
        assert_eq!(tf("Host: {}", &[&"pc"]), "Host: pc");
    }

    #[test]
    fn every_translation_keeps_its_placeholders() {
        for (english, spanish) in TABLE {
            assert_eq!(
                english.matches("{}").count(),
                spanish.matches("{}").count(),
                "{english}"
            );
        }
        assert_eq!(table().len(), TABLE.len(), "an English text appears twice");
    }

    /// Every text wrapped in the code has its row in the table, so no screen
    /// falls back to English by accident.
    #[test]
    fn every_wrapped_text_is_translated() {
        for source in [
            include_str!("app.rs"),
            include_str!("dialogs.rs"),
            include_str!("session.rs"),
            include_str!("model.rs"),
        ] {
            for start in ["t(\"", "tf(\n", "tf(\""] {
                for (at, _) in source.match_indices(start) {
                    if source[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.')
                    {
                        continue;
                    }
                    let rest = &source[at + start.len() - 1..];
                    let Some(open) = rest.find('"') else { continue };
                    let text = read_literal(&rest[open + 1..]);
                    assert!(
                        table().contains_key(text.as_str()),
                        "not translated: {text}"
                    );
                }
            }
        }
    }

    /// A Rust string literal's value, up to its closing quote.
    fn read_literal(source: &str) -> String {
        let mut out = String::new();
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('\n') => while chars.next_if(|c| c.is_whitespace()).is_some() {},
                    Some('n') => out.push('\n'),
                    Some(other) => out.push(other),
                    None => break,
                },
                other => out.push(other),
            }
        }
        out
    }
}
