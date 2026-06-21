use std::{
    error::Error,
    fs,
    io::{self, stdout},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

type AppResult<T> = Result<T, Box<dyn Error>>;
type Tui = Terminal<CrosstermBackend<io::Stdout>>;

const REPORT_TEMPLATE: &str = include_str!("../../rapport.tex");
const TITLE_PAGE_TEMPLATE: &str = include_str!("../../other/titlepage.tex");
const SECTION_TEMPLATE: &str = include_str!("../../sections/section.tex");
const WORKFLOW_TEMPLATE: &str = include_str!("../../.github/workflows/build.yml");
const UM_LOGO: &[u8] = include_bytes!("../../images/UM.png");
const DO_LOGO: &[u8] = include_bytes!("../../images/DO.png");

const PROJECT_GITIGNORE: &str = r#"/.claude/
/rapport.aux
/rapport.bbl
/rapport.bcf
/rapport.blg
/rapport.fdb_latexmk
/rapport.fls
/rapport.log
/rapport.out
/rapport.pdf
/rapport.run.xml
/rapport.synctex.gz
/rapport.toc
sections/*.aux
"#;

const PROJECT_NAME: usize = 0;
const REPORT_TITLE: usize = 1;
const REPORT_TYPE: usize = 2;
const AUTHOR: usize = 3;
const PROGRAM: usize = 4;
const ACADEMIC_YEAR: usize = 5;
const PARENT_DIRECTORY: usize = 6;
const FOLDER_NAME: usize = 7;
const GIT_REMOTE: usize = 8;
const INITIAL_COMMIT: usize = 9;

#[derive(Debug)]
struct Field {
    label: &'static str,
    value: String,
    placeholder: &'static str,
}

#[derive(Clone, Debug)]
enum PickerEntry {
    SelectCurrent,
    Parent(PathBuf),
    Directory(PathBuf),
}

impl PickerEntry {
    fn label(&self) -> String {
        match self {
            Self::SelectCurrent => "[ Select this directory ]".into(),
            Self::Parent(_) => "../".into(),
            Self::Directory(path) => format!(
                "{}/",
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into()
                )
            ),
        }
    }
}

#[derive(Debug)]
struct DirectoryPicker {
    current: PathBuf,
    entries: Vec<PickerEntry>,
    selected: usize,
    error: Option<String>,
}

impl DirectoryPicker {
    fn new(path: &Path) -> AppResult<Self> {
        if !path.is_dir() {
            return Err(input_error(format!(
                "Parent directory does not exist: {}",
                path.display()
            )));
        }

        let mut picker = Self {
            current: fs::canonicalize(path)?,
            entries: Vec::new(),
            selected: 0,
            error: None,
        };
        picker.refresh();
        Ok(picker)
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.entries.push(PickerEntry::SelectCurrent);
        if let Some(parent) = self.current.parent() {
            self.entries.push(PickerEntry::Parent(parent.to_path_buf()));
        }

        match fs::read_dir(&self.current) {
            Ok(entries) => {
                let mut directories: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect();
                directories.sort_by_key(|path| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase()
                });
                self.entries
                    .extend(directories.into_iter().map(PickerEntry::Directory));
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }

        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        self.selected = (self.selected + 1).min(self.entries.len().saturating_sub(1));
    }

    fn enter_selected(&mut self) -> Option<PathBuf> {
        match self.entries.get(self.selected).cloned() {
            Some(PickerEntry::SelectCurrent) => Some(self.current.clone()),
            Some(PickerEntry::Parent(path) | PickerEntry::Directory(path)) => {
                self.open(path);
                None
            }
            None => None,
        }
    }

    fn go_to_parent(&mut self) {
        if let Some(parent) = self.current.parent().map(Path::to_path_buf) {
            self.open(parent);
        }
    }

    fn open(&mut self, path: PathBuf) {
        match fs::canonicalize(&path) {
            Ok(path) if path.is_dir() => {
                self.current = path;
                self.selected = 0;
                self.refresh();
            }
            Ok(_) => self.error = Some(format!("Not a directory: {}", path.display())),
            Err(error) => self.error = Some(error.to_string()),
        }
    }
}

#[derive(Debug)]
struct GenerationOutcome {
    path: PathBuf,
    remote: Option<String>,
    warnings: Vec<String>,
}

struct App {
    fields: Vec<Field>,
    cursors: Vec<usize>,
    active: usize,
    create_initial_commit: bool,
    folder_follows_name: bool,
    directory_picker: Option<DirectoryPicker>,
    status: Option<(String, bool)>,
    outcome: Option<GenerationOutcome>,
    quit: bool,
}

impl App {
    fn new() -> Self {
        let current_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let fields = vec![
            Field {
                label: "Repository name",
                value: "my-latex-project".into(),
                placeholder: "my-latex-project",
            },
            Field {
                label: "Report title",
                value: "My Project".into(),
                placeholder: "Project title",
            },
            Field {
                label: "Report type",
                value: "Project Report".into(),
                placeholder: "Project Report",
            },
            Field {
                label: "Author",
                value: "First LAST".into(),
                placeholder: "First LAST",
            },
            Field {
                label: "Program",
                value: "Program".into(),
                placeholder: "Program",
            },
            Field {
                label: "Academic year",
                value: "20XX--20XX".into(),
                placeholder: "2026--2027",
            },
            Field {
                label: "Parent directory",
                value: current_directory.display().to_string(),
                placeholder: ".",
            },
            Field {
                label: "Folder name",
                value: "my-latex-project".into(),
                placeholder: "my-latex-project",
            },
            Field {
                label: "Git remote (optional)",
                value: String::new(),
                placeholder: "git@github.com:user/repository.git",
            },
            Field {
                label: "Create initial commit",
                value: String::new(),
                placeholder: "",
            },
        ];

        let cursors = fields.iter().map(|field| field.value.len()).collect();

        Self {
            fields,
            cursors,
            active: 0,
            create_initial_commit: true,
            folder_follows_name: true,
            directory_picker: None,
            status: None,
            outcome: None,
            quit: false,
        }
    }

    fn run(&mut self, terminal: &mut Tui) -> AppResult<()> {
        while !self.quit {
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key);
            }
        }

        Ok(())
    }

    fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 60 || area.height < 19 {
            let warning = Paragraph::new("Terminal too small. Resize it to at least 60x19.")
                .style(Style::default().fg(Color::Yellow))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Polytech LaTeX Generator "),
                )
                .wrap(Wrap { trim: true });
            frame.render_widget(warning, area);
            return;
        }

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(12),
                Constraint::Length(4),
            ])
            .split(area);

        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                "Polytech LaTeX Generator",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" — create a customized report repository"),
        ]))
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(header, layout[0]);

        self.render_form(frame, layout[1]);

        let status = self.status.as_ref().map_or_else(
            || {
                if self.active == PARENT_DIRECTORY {
                    "Type a path or press Enter to browse directories. Tab: next".to_string()
                } else {
                    "Tab/Shift+Tab: navigate  Enter: next/create  Ctrl+G: create  Esc: quit"
                        .to_string()
                }
            },
            |(message, _)| message.clone(),
        );
        let status_color = self
            .status
            .as_ref()
            .map_or(Color::DarkGray, |(_, is_error)| {
                if *is_error { Color::Red } else { Color::Green }
            });
        let footer = Paragraph::new(status)
            .style(Style::default().fg(status_color))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Help / status "),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(footer, layout[2]);

        if let Some(picker) = &self.directory_picker {
            self.render_directory_picker(frame, picker);
        }
    }

    fn render_form(&self, frame: &mut Frame, area: Rect) {
        let form_block = Block::default()
            .borders(Borders::ALL)
            .title(" Project settings ");
        let inner = form_block.inner(area);
        frame.render_widget(form_block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Length(1); self.fields.len()])
            .split(inner);

        for (index, field) in self.fields.iter().enumerate() {
            let active = index == self.active && self.directory_picker.is_none();
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(28), Constraint::Min(1)])
                .split(rows[index]);
            let label_style = if active {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            frame.render_widget(
                Paragraph::new(format!(
                    "{}{} ",
                    if active { "> " } else { "  " },
                    field.label
                ))
                .alignment(Alignment::Right)
                .style(label_style),
                columns[0],
            );

            let (value, style) = if index == INITIAL_COMMIT {
                (
                    if self.create_initial_commit {
                        "[x] Yes"
                    } else {
                        "[ ] No"
                    },
                    Style::default().fg(Color::White),
                )
            } else if field.value.is_empty() {
                (field.placeholder, Style::default().fg(Color::DarkGray))
            } else {
                (field.value.as_str(), Style::default().fg(Color::White))
            };

            let inner_width = columns[1].width;
            let cursor_width = if active && index != INITIAL_COMMIT {
                UnicodeWidthStr::width(&field.value[..self.cursors[index]]) as u16
            } else {
                0
            };
            let horizontal_scroll = cursor_width.saturating_sub(inner_width.saturating_sub(1));

            frame.render_widget(
                Paragraph::new(value)
                    .style(style)
                    .scroll((0, horizontal_scroll)),
                columns[1],
            );

            if active && index != INITIAL_COMMIT {
                frame.set_cursor_position((
                    columns[1].x + cursor_width.saturating_sub(horizontal_scroll),
                    columns[1].y,
                ));
            }
        }
    }

    fn render_directory_picker(&self, frame: &mut Frame, picker: &DirectoryPicker) {
        let screen = frame.area();
        let area = Rect {
            x: screen.x + 4,
            y: screen.y + 2,
            width: screen.width.saturating_sub(8),
            height: screen.height.saturating_sub(4),
        };
        frame.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Select parent directory ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);

        frame.render_widget(
            Paragraph::new(picker.current.display().to_string())
                .style(Style::default().fg(Color::Cyan))
                .wrap(Wrap { trim: false }),
            layout[0],
        );

        let visible_count = usize::from(layout[1].height);
        let offset = picker
            .selected
            .saturating_sub(visible_count.saturating_sub(1));
        let lines: Vec<Line<'_>> = picker
            .entries
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible_count)
            .map(|(index, entry)| {
                let style = if index == picker.selected {
                    Style::default().fg(Color::Black).bg(Color::Yellow)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::styled(format!(" {} ", entry.label()), style)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), layout[1]);

        let (help, color) = picker.error.as_ref().map_or(
            (
                "↑/↓: navigate  Enter: open/select  Backspace: parent  S: select current  Esc: cancel",
                Color::DarkGray,
            ),
            |error| (error.as_str(), Color::Red),
        );
        frame.render_widget(
            Paragraph::new(help)
                .style(Style::default().fg(color))
                .wrap(Wrap { trim: true }),
            layout[2],
        );
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.directory_picker.is_some() {
            self.handle_directory_picker_key(key);
            return;
        }

        self.status = None;

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => self.quit = true,
                KeyCode::Char('g') => self.submit(),
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => self.quit = true,
            KeyCode::Tab | KeyCode::Down => self.focus_next(),
            KeyCode::BackTab | KeyCode::Up => self.focus_previous(),
            KeyCode::Enter => {
                if self.active == PARENT_DIRECTORY {
                    self.open_directory_picker();
                } else if self.active == INITIAL_COMMIT {
                    self.submit();
                } else {
                    self.focus_next();
                }
            }
            KeyCode::Char(' ') if self.active == INITIAL_COMMIT => {
                self.create_initial_commit = !self.create_initial_commit;
            }
            KeyCode::Char(character) if self.active != INITIAL_COMMIT => {
                self.insert_character(character);
            }
            KeyCode::Backspace if self.active != INITIAL_COMMIT => self.backspace(),
            KeyCode::Delete if self.active != INITIAL_COMMIT => self.delete(),
            KeyCode::Left if self.active != INITIAL_COMMIT => self.move_cursor_left(),
            KeyCode::Right if self.active != INITIAL_COMMIT => self.move_cursor_right(),
            KeyCode::Home if self.active != INITIAL_COMMIT => self.cursors[self.active] = 0,
            KeyCode::End if self.active != INITIAL_COMMIT => {
                self.cursors[self.active] = self.fields[self.active].value.len();
            }
            _ => {}
        }
    }

    fn handle_directory_picker_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }

        match key.code {
            KeyCode::Esc => self.directory_picker = None,
            KeyCode::Up => self.directory_picker.as_mut().unwrap().move_up(),
            KeyCode::Down => self.directory_picker.as_mut().unwrap().move_down(),
            KeyCode::Home => self.directory_picker.as_mut().unwrap().selected = 0,
            KeyCode::End => {
                let picker = self.directory_picker.as_mut().unwrap();
                picker.selected = picker.entries.len().saturating_sub(1);
            }
            KeyCode::Backspace | KeyCode::Left => {
                self.directory_picker.as_mut().unwrap().go_to_parent();
            }
            KeyCode::Char('s' | 'S') => {
                let path = self.directory_picker.as_ref().unwrap().current.clone();
                self.select_parent_directory(path);
            }
            KeyCode::Enter | KeyCode::Right => {
                let selected = self.directory_picker.as_mut().unwrap().enter_selected();
                if let Some(path) = selected {
                    self.select_parent_directory(path);
                }
            }
            _ => {}
        }
    }

    fn open_directory_picker(&mut self) {
        let entered_path = PathBuf::from(self.fields[PARENT_DIRECTORY].value.trim());
        let path = if entered_path.is_absolute() {
            entered_path
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(entered_path)
        };

        match DirectoryPicker::new(&path) {
            Ok(picker) => self.directory_picker = Some(picker),
            Err(error) => self.status = Some((error.to_string(), true)),
        }
    }

    fn select_parent_directory(&mut self, path: PathBuf) {
        self.fields[PARENT_DIRECTORY].value = path.display().to_string();
        self.cursors[PARENT_DIRECTORY] = self.fields[PARENT_DIRECTORY].value.len();
        self.directory_picker = None;
        self.active = FOLDER_NAME;
        self.status = Some(("Parent directory selected.".into(), false));
    }

    fn focus_next(&mut self) {
        self.active = (self.active + 1) % self.fields.len();
    }

    fn focus_previous(&mut self) {
        self.active = self.active.checked_sub(1).unwrap_or(self.fields.len() - 1);
    }

    fn insert_character(&mut self, character: char) {
        if character == '\n' || character == '\r' {
            return;
        }

        let cursor = self.cursors[self.active];
        self.fields[self.active].value.insert(cursor, character);
        self.cursors[self.active] += character.len_utf8();
        self.after_edit();
    }

    fn backspace(&mut self) {
        let cursor = self.cursors[self.active];
        if cursor == 0 {
            return;
        }

        let previous = previous_boundary(&self.fields[self.active].value, cursor);
        self.fields[self.active].value.drain(previous..cursor);
        self.cursors[self.active] = previous;
        self.after_edit();
    }

    fn delete(&mut self) {
        let cursor = self.cursors[self.active];
        let value = &mut self.fields[self.active].value;
        if cursor == value.len() {
            return;
        }

        let next = next_boundary(value, cursor);
        value.drain(cursor..next);
        self.after_edit();
    }

    fn move_cursor_left(&mut self) {
        self.cursors[self.active] =
            previous_boundary(&self.fields[self.active].value, self.cursors[self.active]);
    }

    fn move_cursor_right(&mut self) {
        self.cursors[self.active] =
            next_boundary(&self.fields[self.active].value, self.cursors[self.active]);
    }

    fn after_edit(&mut self) {
        if self.active == FOLDER_NAME {
            self.folder_follows_name = false;
        }

        if self.active == PROJECT_NAME && self.folder_follows_name {
            self.fields[FOLDER_NAME].value = slugify(&self.fields[PROJECT_NAME].value);
            self.cursors[FOLDER_NAME] = self.fields[FOLDER_NAME].value.len();
        }
    }

    fn submit(&mut self) {
        match self.generate() {
            Ok(outcome) => {
                self.status = Some(("Project created successfully.".into(), false));
                self.outcome = Some(outcome);
                self.quit = true;
            }
            Err(error) => {
                self.status = Some((error.to_string(), true));
            }
        }
    }

    fn generate(&self) -> AppResult<GenerationOutcome> {
        let project_name = self.fields[PROJECT_NAME].value.trim();
        let parent_directory = self.fields[PARENT_DIRECTORY].value.trim();
        let folder_name = self.fields[FOLDER_NAME].value.trim();

        if project_name.is_empty() {
            return Err(input_error("Repository name is required."));
        }
        if parent_directory.is_empty() {
            return Err(input_error("Parent directory is required."));
        }
        if folder_name.is_empty() {
            return Err(input_error("Folder name is required."));
        }
        if folder_name == "."
            || folder_name == ".."
            || folder_name.contains('/')
            || folder_name.contains('\\')
        {
            return Err(input_error(
                "Folder name must be a single directory name without path separators.",
            ));
        }
        if self.fields[REPORT_TITLE].value.trim().is_empty() {
            return Err(input_error("Report title is required."));
        }
        if self.fields[..=GIT_REMOTE]
            .iter()
            .any(|field| field.value.contains(['\n', '\r']))
        {
            return Err(input_error("Fields cannot contain line breaks."));
        }

        let parent = PathBuf::from(parent_directory);
        if !parent.is_dir() {
            return Err(input_error(format!(
                "Parent directory does not exist: {}",
                parent.display()
            )));
        }

        let path = parent.join(folder_name);
        if path.exists() {
            return Err(input_error(format!(
                "Destination already exists: {}",
                path.display()
            )));
        }

        ensure_git_available()?;
        write_project_files(
            &path,
            ProjectMetadata {
                project_name,
                report_title: self.fields[REPORT_TITLE].value.trim(),
                report_type: self.fields[REPORT_TYPE].value.trim(),
                author: self.fields[AUTHOR].value.trim(),
                program: self.fields[PROGRAM].value.trim(),
                academic_year: self.fields[ACADEMIC_YEAR].value.trim(),
            },
        )?;

        run_git(&path, &["init", "-b", "main"])?;

        let remote = self.fields[GIT_REMOTE].value.trim();
        let mut warnings = Vec::new();
        let configured_remote = if remote.is_empty() {
            None
        } else {
            match run_git(&path, &["remote", "add", "origin", remote]) {
                Ok(()) => Some(remote.to_string()),
                Err(error) => {
                    warnings.push(format!("Could not configure origin: {error}"));
                    None
                }
            }
        };

        if self.create_initial_commit
            && let Err(error) = run_git(&path, &["add", "--all"])
                .and_then(|_| run_git(&path, &["commit", "-m", "Initial commit"]))
        {
            warnings.push(format!("Could not create the initial commit: {error}"));
        }

        Ok(GenerationOutcome {
            path: fs::canonicalize(&path).unwrap_or(path),
            remote: configured_remote,
            warnings,
        })
    }
}

struct ProjectMetadata<'a> {
    project_name: &'a str,
    report_title: &'a str,
    report_type: &'a str,
    author: &'a str,
    program: &'a str,
    academic_year: &'a str,
}

fn main() -> AppResult<()> {
    let mut terminal = initialize_terminal()?;
    let mut app = App::new();
    let run_result = app.run(&mut terminal);
    let restore_result = restore_terminal(&mut terminal);

    run_result?;
    restore_result?;

    if let Some(outcome) = app.outcome {
        println!("Created project at {}", outcome.path.display());
        if let Some(remote) = outcome.remote {
            println!("Configured origin: {remote}");
        }
        for warning in outcome.warnings {
            eprintln!("Warning: {warning}");
        }
    }

    Ok(())
}

fn initialize_terminal() -> AppResult<Tui> {
    enable_raw_mode()?;
    let mut output = stdout();
    execute!(output, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(output))?)
}

fn restore_terminal(terminal: &mut Tui) -> AppResult<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn write_project_files(path: &Path, metadata: ProjectMetadata<'_>) -> AppResult<()> {
    fs::create_dir_all(path.join(".github/workflows"))?;
    fs::create_dir_all(path.join("images"))?;
    fs::create_dir_all(path.join("other"))?;
    fs::create_dir_all(path.join("sections"))?;

    let report = customize_report(REPORT_TEMPLATE, &metadata)?;
    fs::write(path.join("rapport.tex"), report)?;
    fs::write(path.join("other/titlepage.tex"), TITLE_PAGE_TEMPLATE)?;
    fs::write(path.join("sections/section.tex"), SECTION_TEMPLATE)?;
    fs::write(path.join("images/UM.png"), UM_LOGO)?;
    fs::write(path.join("images/DO.png"), DO_LOGO)?;
    fs::write(path.join(".github/workflows/build.yml"), WORKFLOW_TEMPLATE)?;
    fs::write(path.join(".gitignore"), PROJECT_GITIGNORE)?;
    fs::write(
        path.join("README.md"),
        generated_readme(metadata.project_name),
    )?;

    Ok(())
}

fn customize_report(template: &str, metadata: &ProjectMetadata<'_>) -> AppResult<String> {
    let replacements = [
        ("reporttype", metadata.report_type),
        ("reporttitle", metadata.report_title),
        ("reportauthor", metadata.author),
        ("reportprogram", metadata.program),
        ("academicyear", metadata.academic_year),
    ];

    let mut report = template.to_string();
    for (command, value) in replacements {
        report = replace_latex_command(&report, command, &latex_escape(value))?;
    }
    Ok(report)
}

fn replace_latex_command(source: &str, command: &str, value: &str) -> AppResult<String> {
    let marker = format!("\\newcommand{{\\{command}}}");
    let mut found = false;
    let mut output = String::with_capacity(source.len() + value.len());

    for line in source.split_inclusive('\n') {
        if line.trim_start().starts_with(&marker) {
            output.push_str(&format!("{marker}{{{value}}}\n"));
            found = true;
        } else {
            output.push_str(line);
        }
    }

    if !found {
        return Err(input_error(format!(
            "Template command not found: \\{command}"
        )));
    }

    Ok(output)
}

fn latex_escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        escaped.push_str(match character {
            '\\' => "\\textbackslash{}",
            '{' => "\\{",
            '}' => "\\}",
            '#' => "\\#",
            '$' => "\\$",
            '%' => "\\%",
            '&' => "\\&",
            '_' => "\\_",
            '^' => "\\textasciicircum{}",
            '~' => "\\textasciitilde{}",
            _ => {
                escaped.push(character);
                continue;
            }
        });
    }
    escaped
}

fn generated_readme(project_name: &str) -> String {
    format!(
        "# {project_name}\n\nGenerated with the [Polytech LaTeX Template](https://github.com/axelfrache/polytech-latex-template).\n\n## Build\n\n```bash\nlatexmk -pdf rapport.tex\n```\n"
    )
}

fn ensure_git_available() -> AppResult<()> {
    let output = Command::new("git").arg("--version").output()?;
    if !output.status.success() {
        return Err(input_error("Git is required to create the repository."));
    }
    Ok(())
}

fn run_git(directory: &Path, arguments: &[&str]) -> AppResult<()> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    let details = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(input_error(if details.is_empty() {
        format!("git {} failed", arguments.join(" "))
    } else {
        details
    }))
}

fn previous_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map_or(value.len(), |(index, _)| cursor + index)
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;

    for character in value.trim().chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }

    if slug.is_empty() {
        "latex-project".into()
    } else {
        slug
    }
}

fn input_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn slugifies_project_names() {
        assert_eq!(slugify("My LaTeX Project"), "my-latex-project");
        assert_eq!(slugify("  Project___2026  "), "project-2026");
    }

    #[test]
    fn escapes_latex_metadata() {
        assert_eq!(latex_escape("R&D_2026"), "R\\&D\\_2026");
    }

    #[test]
    fn customizes_all_metadata() {
        let metadata = ProjectMetadata {
            project_name: "sample",
            report_title: "Custom Title",
            report_type: "Internship Report",
            author: "Ada Lovelace",
            program: "Computer Science",
            academic_year: "2026--2027",
        };

        let report = customize_report(REPORT_TEMPLATE, &metadata).unwrap();
        assert!(report.contains("\\newcommand{\\reporttitle}{Custom Title}"));
        assert!(report.contains("\\newcommand{\\reportauthor}{Ada Lovelace}"));
        assert!(!report.contains("Titre du projet"));
    }

    #[test]
    fn writes_a_complete_project() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "polytech-latex-generator-{}-{unique}",
            std::process::id()
        ));
        let metadata = ProjectMetadata {
            project_name: "sample-report",
            report_title: "Sample Report",
            report_type: "Project Report",
            author: "Ada Lovelace",
            program: "Computer Science",
            academic_year: "2026--2027",
        };

        write_project_files(&path, metadata).unwrap();

        assert!(path.join("rapport.tex").is_file());
        assert!(path.join("images/UM.png").is_file());
        assert!(path.join(".github/workflows/build.yml").is_file());
        assert!(
            fs::read_to_string(path.join("README.md"))
                .unwrap()
                .contains("# sample-report")
        );

        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn initializes_git_repository_with_remote() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder_name = format!("polytech-latex-git-{}-{unique}", std::process::id());
        let path = std::env::temp_dir().join(&folder_name);
        let remote = "git@github.com:example/sample-report.git";
        let mut app = App::new();
        app.fields[PARENT_DIRECTORY].value = std::env::temp_dir().display().to_string();
        app.fields[FOLDER_NAME].value = folder_name;
        app.fields[GIT_REMOTE].value = remote.into();
        app.create_initial_commit = false;

        let outcome = app.generate().unwrap();
        let configured_remote = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&path)
            .output()
            .unwrap();

        assert!(path.join(".git").is_dir());
        assert_eq!(outcome.remote.as_deref(), Some(remote));
        assert_eq!(
            String::from_utf8(configured_remote.stdout).unwrap().trim(),
            remote
        );

        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn directory_picker_navigates_and_selects_a_directory() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "polytech-latex-picker-{}-{unique}",
            std::process::id()
        ));
        let child = root.join("reports");
        fs::create_dir_all(&child).unwrap();

        let mut picker = DirectoryPicker::new(&root).unwrap();
        let child_index = picker
            .entries
            .iter()
            .position(|entry| matches!(entry, PickerEntry::Directory(path) if path == &child))
            .unwrap();
        picker.selected = child_index;

        assert!(picker.enter_selected().is_none());
        assert_eq!(picker.current, fs::canonicalize(&child).unwrap());
        assert_eq!(
            picker.enter_selected(),
            Some(fs::canonicalize(&child).unwrap())
        );

        fs::remove_dir_all(root).unwrap();
    }
}
