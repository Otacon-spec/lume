//! Lettura dei profili JetBrains; nessun socket, segreto o modifica a .idea.

#[derive(Clone, PartialEq, Eq)]
pub struct Connection {
    pub uuid: String,
    pub label: String,
    pub oracle: bool,
    pub host: String,
    pub database: String,
    pub user: String,
}

#[derive(Clone, Default)]
pub struct Report {
    pub connections: Vec<Connection>,
    pub skipped: Vec<String>,
}

const MAX_BYTES: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 256;

impl std::fmt::Debug for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdeaImport")
            .field("connections", &self.connections.len())
            .field("skipped", &self.skipped.len())
            .finish()
    }
}

fn document(xml: &str) -> Result<roxmltree::Document<'_>, &'static str> {
    if xml.len() > MAX_BYTES {
        return Err("Configurazione .idea troppo grande (massimo 1 MiB)");
    }
    roxmltree::Document::parse(xml).map_err(|_| "XML .idea non valido o DTD non consentito")
}

fn value<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
        .map(str::trim)
}

fn sources<'a>(
    doc: &'a roxmltree::Document<'a>,
) -> Result<Vec<roxmltree::Node<'a, 'a>>, &'static str> {
    let nodes: Vec<_> = doc
        .descendants()
        .filter(|n| {
            n.has_tag_name("data-source")
                && n.parent().is_some_and(|p| {
                    p.has_tag_name("component")
                        && matches!(
                            p.attribute("name"),
                            Some("DataSourceManager" | "DataSourceManagerImpl")
                        )
                })
        })
        .collect();
    if nodes.len() > MAX_CONNECTIONS {
        return Err("Troppe connessioni .idea (massimo 256)");
    }
    let mut ids = std::collections::HashSet::new();
    for n in &nodes {
        let id = n
            .attribute("uuid")
            .filter(|s| !s.is_empty())
            .ok_or("Connessione .idea senza UUID")?;
        if !ids.insert(id) {
            return Err("UUID duplicato nella configurazione .idea");
        }
    }
    Ok(nodes)
}

pub fn parse(xml: &str, local: Option<&str>) -> Result<Report, &'static str> {
    let doc = document(xml)?;
    let local = local.map(document).transpose()?;
    let local_nodes = local.as_ref().map(sources).transpose()?.unwrap_or_default();
    let mut report = Report::default();
    for node in sources(&doc)? {
        let uuid = node.attribute("uuid").ok_or("Connessione senza UUID")?;
        let local_node = local_nodes
            .iter()
            .find(|n| n.attribute("uuid") == Some(uuid))
            .copied();
        let read = |key| {
            local_node
                .and_then(|n| value(n, key))
                .or_else(|| value(node, key))
        };
        let user = read("user-name").unwrap_or_default();
        let url = read("jdbc-url").unwrap_or_default();
        // Le proprietà aggiuntive non rappresentabili devono essere visibili,
        // mai eliminate per ottenere una connessione apparentemente valida.
        let unsupported = [Some(node), local_node].into_iter().flatten().any(|n| {
            n.children().any(|c| {
                matches!(
                    c.tag_name().name(),
                    "ssh-tunnel" | "ssh-settings" | "ssl-config" | "driver-properties"
                ) || (c.has_tag_name("jdbc-additional-properties")
                    && c.children()
                        .filter(|p| p.has_tag_name("property"))
                        .any(|p| {
                            let name = p.attribute("name").unwrap_or_default();
                            let value = p.attribute("value").unwrap_or_default();
                            !(value.is_empty()
                                || name == "com.intellij.clouds.kubernetes.db.enabled"
                                    && value.eq_ignore_ascii_case("false"))
                        }))
            })
        });
        let auth_ok = read("auth-provider").is_none_or(|a| matches!(a, "user-pass" | "password"));
        let target = if unsupported || !auth_ok {
            None
        } else {
            target(url)
        };
        if let Some((oracle, host, database, url_user)) = target {
            report.connections.push(Connection {
                uuid: uuid.to_owned(),
                label: node
                    .attribute("name")
                    .unwrap_or("Connessione .idea")
                    .to_owned(),
                oracle,
                host,
                database,
                user: if user.is_empty() {
                    url_user
                } else {
                    user.to_owned()
                },
            });
        } else {
            report.skipped.push(format!(
                "Connessione {}: motore, autenticazione o opzioni non supportati",
                report.connections.len() + report.skipped.len() + 1
            ));
        }
    }
    Ok(report)
}

fn authority(host: &str) -> bool {
    let (name, port) = host
        .split_once(':')
        .map_or((host, None), |(n, p)| (n, Some(p)));
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        && port.is_none_or(|p| p.parse::<u16>().is_ok_and(|v| v != 0))
}

fn target(url: &str) -> Option<(bool, String, String, String)> {
    if let Some(rest) = url.strip_prefix("jdbc:sqlserver://") {
        let mut parts = rest.split(';');
        let host = parts.next()?;
        if !authority(host) {
            return None;
        }
        let (mut database, mut user) = (String::new(), String::new());
        let mut seen = std::collections::HashSet::new();
        for property in parts.filter(|p| !p.is_empty()) {
            let (key, value) = property.split_once('=')?;
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            if !seen.insert(key.clone()) {
                return None;
            }
            match key.as_str() {
                "databasename" => database = value.to_owned(),
                "user" | "username" => user = value.to_owned(),
                "password" => {} // Il vault JetBrains non viene letto né copiato.
                "encrypt" if value.eq_ignore_ascii_case("true") => {}
                "trustservercertificate" if value.eq_ignore_ascii_case("false") => {}
                "integratedsecurity" if value.eq_ignore_ascii_case("false") => {}
                "authentication" if value.eq_ignore_ascii_case("SqlPassword") => {}
                _ => return None,
            }
        }
        return Some((false, host.to_owned(), database, user));
    }
    let rest = url.strip_prefix("jdbc:oracle:thin:@")?;
    let (scheme, rest) = if let Some(r) = rest.strip_prefix("tcps://") {
        ("tcps", r)
    } else if let Some(r) = rest.strip_prefix("tcp://") {
        ("tcp", r)
    } else if let Some(r) = rest.strip_prefix("//") {
        ("tcp", r)
    } else {
        ("tcp", rest)
    };
    let (host, service) = rest.split_once('/')?;
    if !authority(host)
        || service.is_empty()
        || !service
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return None;
    }
    Some((
        true,
        format!("{scheme}://{host}"),
        service.to_owned(),
        String::new(),
    ))
}

/// Legge al massimo 1 MiB per file, anche se cresce durante la lettura.
/// O_NONBLOCK impedisce a una FIFO sostituita in corsa di bloccare il worker.
fn read_file(path: &std::path::Path) -> Result<Option<String>, &'static str> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Impossibile leggere la configurazione .idea"),
    };
    let metadata = file
        .metadata()
        .map_err(|_| "Impossibile verificare il file .idea")?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err("File .idea non regolare o troppo grande");
    }
    let mut xml = String::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_string(&mut xml)
        .map_err(|_| "Configurazione .idea non leggibile come UTF-8")?;
    if xml.len() > MAX_BYTES {
        return Err("Configurazione .idea troppo grande");
    }
    Ok(Some(xml))
}

pub fn load(path: &std::path::Path) -> Result<Report, &'static str> {
    let file = if path.is_dir() {
        path.join("dataSources.xml")
    } else {
        path.to_owned()
    };
    let Some(xml) = read_file(&file)? else {
        return Ok(Report::default());
    };
    let local_path = file
        .parent()
        .ok_or("Percorso .idea non valido")?
        .join("dataSources.local.xml");
    let local = read_file(&local_path)?;
    parse(&xml, local.as_deref())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    fn source(url: &str) -> String {
        format!(
            "<project><component name='DataSourceManager'><data-source name='Work' uuid='a'><jdbc-url>{url}</jdbc-url><user-name>alice</user-name></data-source></component></project>"
        )
    }

    #[test]
    fn proprieta_jdbc_non_rappresentabile_viene_segnalata() {
        let xml = source("jdbc:sqlserver://localhost").replace("</data-source>", "<jdbc-additional-properties><property name='trustServerCertificate' value='true'/></jdbc-additional-properties></data-source>");
        let report = parse(&xml, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(report.connections.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn riferimento_fotografico_non_diventa_configurazione_operativa() {
        let xml = include_str!("../tests/fixtures/idea-partial.xml");
        let report = parse(xml, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(report.connections.is_empty());
        assert_eq!(report.skipped.len(), 5);
    }

    #[test]
    fn struttura_foto_completa_con_uuid_distinti_preserva_omonimi() {
        let xml = include_str!("../tests/fixtures/idea-partial.xml")
            .replace("SUFFISSO_NON_LEGGIBILE", "trustServerCertificate=false");
        let report = parse(&xml, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(report.connections.len(), 5);
        assert_eq!(report.connections[2].label, report.connections[4].label);
        assert_ne!(report.connections[2].uuid, report.connections[4].uuid);
    }

    #[test]
    fn sql_server_e_merge_locale_per_uuid() {
        let xml = source("jdbc:sqlserver://db.example:1444;databaseName=work;encrypt=true");
        let local = "<project><component name='DataSourceManagerImpl'><data-source uuid='a'><user-name>bob</user-name></data-source></component></project>";
        let report = parse(&xml, Some(local)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(report.connections.len(), 1);
        let c = &report.connections[0];
        assert_eq!(
            (&*c.host, &*c.database, &*c.user),
            ("db.example:1444", "work", "bob")
        );
        assert!(!c.oracle);
    }

    #[test]
    fn oracle_service_preserva_tcp_e_tcps() {
        for (url, host) in [
            (
                "jdbc:oracle:thin:@//db.example:1521/APP",
                "tcp://db.example:1521",
            ),
            (
                "jdbc:oracle:thin:@tcps://db.example:2484/APP",
                "tcps://db.example:2484",
            ),
        ] {
            let report = parse(&source(url), None).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(report.connections.len(), 1);
            assert_eq!(report.connections[0].host, host);
            assert_eq!(report.connections[0].database, "APP");
            assert!(report.connections[0].oracle);
        }
    }

    #[test]
    fn opzioni_non_rappresentabili_non_vengono_perse() {
        for url in [
            "jdbc:mysql://localhost/db",
            "jdbc:oracle:thin:@localhost:1521:SID",
            "jdbc:sqlserver://localhost;integratedSecurity=true",
            "jdbc:sqlserver://localhost;encrypt=false",
            "jdbc:sqlserver://localhost;trustServerCertificate=true",
            "jdbc:sqlserver://localhost;unknownSetting=yes",
        ] {
            let report = parse(&source(url), None).unwrap_or_else(|e| panic!("{e}"));
            assert!(report.connections.is_empty());
            assert_eq!(report.skipped.len(), 1);
            assert!(!report.skipped[0].contains(url));
        }
    }

    #[test]
    fn password_non_entra_nel_profilo_o_negli_errori() {
        let report = parse(
            &source("jdbc:sqlserver://localhost;databaseName=work;password=secret-xyz"),
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(report.connections.len(), 1);
        let c = &report.connections[0];
        assert!(
            !format!("{}{}{}{}{}", c.label, c.host, c.database, c.user, c.uuid)
                .contains("secret-xyz")
        );
        assert!(parse("<password>secret-xyz", None).is_err());
    }

    #[test]
    fn xml_non_valido_dtd_oversize_uuid_duplicati() {
        assert!(parse("<project>", None).is_err());
        assert!(parse("<!DOCTYPE project><project/>", None).is_err());
        assert!(parse(&" ".repeat(1024 * 1024 + 1), None).is_err());
        let xml = source("jdbc:sqlserver://localhost");
        let twice=xml.replace("</component>", "<data-source uuid='a'><jdbc-url>jdbc:sqlserver://other</jdbc-url></data-source></component>");
        assert!(parse(&twice, None).is_err());
    }
}
