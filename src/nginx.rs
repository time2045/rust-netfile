use crate::auth;

fn sanitize_backend(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_' | '[' | ']'))
        .collect();
    if cleaned.is_empty() {
        "127.0.0.1".into()
    } else {
        cleaned
    }
}

pub fn build_nginx_config(backend: &str, prefix: &str, listen_port: &str) -> String {
    let backend = sanitize_backend(backend);
    let prefix = auth::sanitize_prefix(prefix);
    let listen = if listen_port.trim().chars().all(|c| c.is_ascii_digit())
        && !listen_port.trim().is_empty()
    {
        listen_port.trim().to_string()
    } else {
        "9000".into()
    };
    let mut lines: Vec<String> = vec![
        "server {".into(),
        format!("    listen {listen};"),
        String::new(),
    ];
    if prefix.is_empty() {
        lines.push("    location / {".into());
    } else {
        lines.push(format!("    location = {prefix} {{"));
        lines.push(format!("        return 301 {prefix}/;"));
        lines.push("    }".into());
        lines.push(String::new());
        lines.push(format!("    location {prefix}/ {{"));
    }
    lines.push(format!("        proxy_pass http://{backend}/;"));
    if !prefix.is_empty() {
        lines.push(format!(
            "        proxy_set_header X-Forwarded-Prefix {prefix};"
        ));
    }
    lines.push("        proxy_set_header Host $host;".into());
    lines.push("        proxy_set_header X-Real-IP $remote_addr;".into());
    lines.push("        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;".into());
    lines.push("        proxy_http_version 1.1;".into());
    lines.push("        proxy_read_timeout 3600s;".into());
    lines.push("        proxy_send_timeout 3600s;".into());
    lines.push("        client_max_body_size 0;".into());
    lines.push("        proxy_request_buffering off;".into());
    lines.push("    }".into());
    lines.push(String::new());
    lines.push("    gzip on;".into());
    lines.push(
        "    gzip_types text/css application/javascript application/json image/svg+xml;".into(),
    );
    lines.push("    gzip_min_length 2048;".into());
    lines.push("}".into());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nginx_template() {
        let text = build_nginx_config("192.168.1.2:8888", "/file/", "9000");
        assert!(text.contains("proxy_pass http://192.168.1.2:8888/;"));
        assert!(text.contains("X-Forwarded-Prefix /file;"));
        assert!(text.contains("location = /file {"));
    }

    #[test]
    fn root_deployment() {
        let text = build_nginx_config("127.0.0.1:8888", "", "9000");
        assert!(text.contains("location / {"));
        assert!(!text.contains("X-Forwarded-Prefix"));
    }
}
