use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use url::form_urlencoded;

use super::PREFIX;
use super::chart::Zone;
use crate::clock;

const TZ_COOKIE: &str = "opengateway_tz";
const TZ_NAME_COOKIE: &str = "opengateway_tzname";
pub(super) const FLASH_COOKIE: &str = "opengateway_flash";
const FLASH_SECS: i64 = 60;
const FLASH_CHARS: usize = 300;
const MAX_OFFSET: i64 = 14 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Overview,
    Usage,
    Traffic,
    Activity,
    Accounts,
    Models,
    Keys,
}

pub(super) const TABS: [Tab; 7] = [
    Tab::Overview,
    Tab::Usage,
    Tab::Traffic,
    Tab::Activity,
    Tab::Accounts,
    Tab::Models,
    Tab::Keys,
];

impl Tab {
    pub(super) fn path(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Usage => "usage",
            Self::Traffic => "traffic",
            Self::Activity => "activity",
            Self::Accounts => "accounts",
            Self::Models => "models",
            Self::Keys => "keys",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Usage => "Usage",
            Self::Traffic => "Traffic",
            Self::Activity => "Activity",
            Self::Accounts => "Accounts",
            Self::Models => "Models",
            Self::Keys => "Keys",
        }
    }
}

pub(super) struct Link {
    pub(super) label: &'static str,
    pub(super) path: &'static str,
    pub(super) current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FlashTone {
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Flash {
    pub(super) tone: FlashTone,
    pub(super) text: String,
}

impl Flash {
    pub(super) fn done(text: impl Into<String>) -> Self {
        Self {
            tone: FlashTone::Done,
            text: text.into(),
        }
    }

    pub(super) fn failed(text: impl Into<String>) -> Self {
        Self {
            tone: FlashTone::Failed,
            text: text.into(),
        }
    }

    pub(super) fn failed_tone(&self) -> bool {
        self.tone == FlashTone::Failed
    }

    fn encode(&self) -> String {
        let tone = match self.tone {
            FlashTone::Done => "done",
            FlashTone::Failed => "failed",
        };
        form_urlencoded::Serializer::new(String::new())
            .append_pair("tone", tone)
            .append_pair("text", &self.text)
            .finish()
            .replace('&', "|")
    }

    fn decode(value: &str) -> Option<Self> {
        let text = value.replace('|', "&");
        let mut tone = None;
        let mut message = None;
        for (name, value) in form_urlencoded::parse(text.as_bytes()) {
            match name.as_ref() {
                "tone" => {
                    tone = match value.as_ref() {
                        "done" => Some(FlashTone::Done),
                        "failed" => Some(FlashTone::Failed),
                        _ => None,
                    };
                }
                "text" => message = Some(value.into_owned()),
                _ => {}
            }
        }
        Some(Self {
            tone: tone?,
            text: message.filter(|text| !text.is_empty())?,
        })
    }
}

pub(super) struct Frame {
    pub(super) title: String,
    pub(super) tabs: Vec<Link>,
    pub(super) flash: Option<Flash>,
    pub(super) zone: String,
}

pub(super) struct Ctx {
    pub(super) now: i64,
    pub(super) zone: Zone,
    pub(super) flash: Option<Flash>,
}

impl Ctx {
    pub(super) fn from_headers(headers: &HeaderMap) -> Self {
        let offset = cookie(headers, TZ_COOKIE)
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|offset| offset.abs() <= MAX_OFFSET)
            .unwrap_or(0);
        let name = cookie(headers, TZ_NAME_COOKIE)
            .map(|value| {
                form_urlencoded::parse(format!("n={value}").as_bytes())
                    .map(|(_, name)| name.into_owned())
                    .collect::<String>()
            })
            .filter(|name| {
                name.len() <= 64
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
            });
        Self {
            now: clock::now(),
            zone: Zone::new(offset, name.as_deref()),
            flash: cookie(headers, FLASH_COOKIE).and_then(|value| Flash::decode(&value)),
        }
    }

    pub(super) fn frame(&self, current: Option<Tab>, title: impl Into<String>) -> Frame {
        Frame {
            title: title.into(),
            tabs: TABS
                .into_iter()
                .map(|tab| Link {
                    label: tab.label(),
                    path: tab.path(),
                    current: Some(tab) == current,
                })
                .collect(),
            flash: self.flash.clone(),
            zone: self.zone.label(self.now),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Ctx {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> {
        std::future::ready(Ok(Self::from_headers(&parts.headers)))
    }
}

pub(super) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('='))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn with_flash(mut response: Response, flash: &Flash) -> Response {
    let mut text: String = flash.text.chars().take(FLASH_CHARS).collect();
    if text.len() < flash.text.len() {
        text.push('…');
    }
    let cookie = format!(
        "{FLASH_COOKIE}={}; Path={PREFIX}; HttpOnly; SameSite=Strict; Max-Age={FLASH_SECS}",
        Flash {
            tone: flash.tone,
            text,
        }
        .encode()
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

pub(super) fn secure_flash(response: &mut Response) {
    let flashes: Vec<HeaderValue> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter(|value| value.as_bytes().starts_with(b"opengateway_flash="))
        .filter_map(|value| {
            HeaderValue::from_str(&format!("{}; Secure", value.to_str().ok()?)).ok()
        })
        .collect();
    if flashes.is_empty() {
        return;
    }
    let others: Vec<HeaderValue> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter(|value| !value.as_bytes().starts_with(b"opengateway_flash="))
        .cloned()
        .collect();
    response.headers_mut().remove(header::SET_COOKIE);
    for value in others.into_iter().chain(flashes) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

pub(super) fn clear_flash(response: &mut Response) {
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "opengateway_flash=; Path=/admin; HttpOnly; SameSite=Strict; Max-Age=0",
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_round_trips_through_a_cookie_value() {
        let flash = Flash::failed("Renamed a; b & c = 100%");
        let value = flash.encode();
        assert!(!value.contains(';') && !value.contains(' ') && !value.contains(','));
        assert!(HeaderValue::from_str(&value).is_ok());
        assert_eq!(Flash::decode(&value), Some(flash));
        assert_eq!(Flash::decode("tone=other|text=x"), None);
    }

    #[test]
    fn long_flashes_are_cut_to_fit_a_cookie() {
        let long = Flash::failed("é".repeat(2000));
        let response = with_flash(Response::default(), &long);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.len() < 4000, "{}", cookie.len());
        let value = cookie
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("opengateway_flash=")
            .unwrap();
        let text = Flash::decode(value).unwrap().text;
        assert_eq!(text.chars().count(), FLASH_CHARS + 1);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn flashes_are_marked_secure_behind_https() {
        let mut response = with_flash(Response::default(), &Flash::done("x"));
        response
            .headers_mut()
            .append(header::SET_COOKIE, HeaderValue::from_static("other=1"));
        secure_flash(&mut response);
        let cookies: Vec<&str> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert_eq!(cookies.len(), 2);
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("opengateway_flash=") && c.ends_with("; Secure"))
        );
        assert!(cookies.contains(&"other=1"));
    }

    #[test]
    fn context_reads_the_browser_offset_and_ignores_nonsense() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=b; opengateway_tz=32400"),
        );
        assert_eq!(Ctx::from_headers(&headers).zone.offset_at(0), 32_400);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("opengateway_tz=999999"),
        );
        assert_eq!(Ctx::from_headers(&headers).zone.offset_at(0), 0);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("opengateway_tz=0; opengateway_tzname=Asia%2FSeoul"),
        );
        assert_eq!(Ctx::from_headers(&headers).zone.offset_at(0), 32_400);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("opengateway_tz=3600; opengateway_tzname=..%2F..%2Fetc"),
        );
        assert_eq!(Ctx::from_headers(&headers).zone.offset_at(0), 3600);
    }
}
