//! Экран логина: чистая форма, без ввода-вывода и сети.
//!
//! Вся логика ввода живёт здесь, чтобы её можно было тестировать без
//! терминала: [`LoginForm`] ничего не знает ни про ratatui, ни про tokio.

/// Поле формы, на котором сейчас фокус.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Field {
    /// Адрес инстанса, например `http://localhost:3000`.
    #[default]
    Base,
    /// Имя пользователя.
    Username,
    /// Пароль (отображается звёздочками).
    Password,
}

impl Field {
    /// Следующее поле по кругу.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Base => Self::Username,
            Self::Username => Self::Password,
            Self::Password => Self::Base,
        }
    }

    /// Предыдущее поле по кругу.
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Base => Self::Password,
            Self::Username => Self::Base,
            Self::Password => Self::Username,
        }
    }

    /// Подпись поля для интерфейса.
    pub fn label(self) -> &'static str {
        match self {
            Self::Base => "Сервер",
            Self::Username => "Логин",
            Self::Password => "Пароль",
        }
    }
}

/// Готовые к отправке учётные данные.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Базовый URL инстанса без завершающего слэша.
    pub base: String,
    /// Имя пользователя.
    pub username: String,
    /// Пароль как есть (без trim).
    pub password: String,
}

/// Состояние формы логина.
#[derive(Debug, Clone)]
pub struct LoginForm {
    base: String,
    username: String,
    password: String,
    focus: Field,
}

impl Default for LoginForm {
    fn default() -> Self {
        Self {
            base: "http://localhost:3000".to_owned(),
            username: String::new(),
            password: String::new(),
            focus: Field::Base,
        }
    }
}

impl LoginForm {
    /// Пустая форма с адресом по умолчанию.
    pub fn new() -> Self {
        Self::default()
    }

    /// Поле с фокусом.
    pub fn focus(&self) -> Field {
        self.focus
    }

    /// Переводит фокус на следующее поле.
    pub fn focus_next(&mut self) {
        self.focus = self.focus.next();
    }

    /// Переводит фокус на предыдущее поле.
    pub fn focus_prev(&mut self) {
        self.focus = self.focus.prev();
    }

    /// Значение поля как есть.
    pub fn value(&self, field: Field) -> &str {
        match field {
            Field::Base => &self.base,
            Field::Username => &self.username,
            Field::Password => &self.password,
        }
    }

    /// Значение поля для показа: пароль маскируется.
    pub fn display_value(&self, field: Field) -> String {
        match field {
            Field::Password => "*".repeat(self.password.chars().count()),
            Field::Base | Field::Username => self.value(field).to_owned(),
        }
    }

    /// Дописывает символ в поле с фокусом. Управляющие символы игнорируются.
    pub fn push_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        self.target_mut().push(c);
    }

    /// Удаляет последний символ поля с фокусом.
    pub fn backspace(&mut self) {
        self.target_mut().pop();
    }

    /// Можно ли отправлять форму.
    pub fn is_submittable(&self) -> bool {
        let base_ok = self.base.starts_with("http://") || self.base.starts_with("https://");
        base_ok && !self.username.trim().is_empty() && !self.password.is_empty()
    }

    /// Забирает учётные данные, если форма валидна.
    ///
    /// Адрес нормализуется: пробелы по краям срезаются, хвостовой `/` убирается.
    pub fn credentials(&self) -> Option<Credentials> {
        if !self.is_submittable() {
            return None;
        }
        let base = self.base.trim().trim_end_matches('/').to_owned();
        Some(Credentials {
            base,
            username: self.username.trim().to_owned(),
            password: self.password.clone(),
        })
    }

    /// Почему форма не отправляется — короткая подсказка для футера.
    pub fn hint(&self) -> &'static str {
        if self.is_submittable() {
            "Enter — подключиться"
        } else {
            "Нужен адрес http(s)://, логин и пароль"
        }
    }

    fn target_mut(&mut self) -> &mut String {
        match self.focus {
            Field::Base => &mut self.base,
            Field::Username => &mut self.username,
            Field::Password => &mut self.password,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_cycles_both_ways() {
        let mut form = LoginForm::new();
        assert_eq!(form.focus(), Field::Base);
        form.focus_next();
        assert_eq!(form.focus(), Field::Username);
        form.focus_next();
        assert_eq!(form.focus(), Field::Password);
        form.focus_next();
        assert_eq!(form.focus(), Field::Base);
        form.focus_prev();
        assert_eq!(form.focus(), Field::Password);
    }

    #[test]
    fn typing_goes_to_focused_field_only() {
        let mut form = LoginForm::new();
        form.focus_next();
        form.push_char('a');
        form.push_char('l');
        assert_eq!(form.value(Field::Username), "al");
        assert!(form.value(Field::Base).starts_with("http://"));
        assert!(form.value(Field::Password).is_empty());
    }

    #[test]
    fn control_chars_are_ignored() {
        let mut form = LoginForm::new();
        form.focus_next();
        form.push_char('\n');
        form.push_char('\t');
        assert!(form.value(Field::Username).is_empty());
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut form = LoginForm::new();
        form.focus_next();
        form.push_char('x');
        form.push_char('y');
        form.backspace();
        assert_eq!(form.value(Field::Username), "x");
        form.backspace();
        form.backspace();
        assert!(form.value(Field::Username).is_empty());
    }

    #[test]
    fn password_is_masked_by_char_count() {
        let mut form = LoginForm::new();
        form.focus_next();
        form.focus_next();
        form.push_char('п');
        form.push_char('а');
        assert_eq!(form.display_value(Field::Password), "**");
    }

    #[test]
    fn form_requires_all_three_fields() {
        let mut form = LoginForm::new();
        assert!(!form.is_submittable());
        form.focus_next();
        form.push_char('u');
        assert!(!form.is_submittable());
        form.focus_next();
        form.push_char('p');
        assert!(form.is_submittable());
    }

    #[test]
    fn non_http_base_is_rejected() {
        let mut form = LoginForm::new();
        // Фокус по умолчанию — на адресе; затираем его и вводим ftp://.
        for _ in 0..form.value(Field::Base).chars().count() {
            form.backspace();
        }
        for c in "ftp://host".chars() {
            form.push_char(c);
        }
        form.focus_next();
        form.push_char('u');
        form.focus_next();
        form.push_char('p');
        assert!(!form.is_submittable());
        assert!(form.credentials().is_none());
    }

    #[test]
    fn credentials_are_trimmed_and_unslashed() {
        let mut form = LoginForm::new();
        form.focus_next();
        form.push_char(' ');
        form.push_char('b');
        form.push_char('o');
        form.push_char('b');
        form.push_char(' ');
        form.focus_next();
        form.push_char('s');
        form.push_char('e');
        form.push_char('c');
        form.push_char('r');
        form.push_char('e');
        form.push_char('t');
        let creds = form.credentials().unwrap();
        assert_eq!(creds.username, "bob");
        assert!(!creds.base.ends_with('/'));
        assert_eq!(creds.password, "secret");
    }
}
