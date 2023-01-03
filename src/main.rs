use ::crossterm::event::*;
use ::crossterm::terminal::ClearType;
use ::crossterm::{cursor, event, execute, queue, style, terminal};
use std::io::{self, stdout, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{cmp, env, fs};

// Cleanup struct is used to disable raw mode
// Called at the start of main() and when it goes
// out of scope, the drop() implmentation is called
struct Cleanup;

const VERSION: f32 = 1.0;

impl Drop for Cleanup {
    fn drop(&mut self) {
        terminal::disable_raw_mode().expect("Couldn't disable raw mode");
        Output::clear_screen().expect("Error clearing screen");
    }
}

// Used to move around the cursor based on
// some user key presses
struct CursorController {
    cursor_x: usize,
    cursor_y: usize,
    screen_columns: usize,
    screen_rows: usize,
    row_offset: usize,
    column_offset: usize,
    render_x: usize,
}

impl CursorController {
    fn new(win_size: (usize, usize)) -> CursorController {
        Self {
            cursor_x: 0,
            cursor_y: 0,
            screen_columns: win_size.0,
            screen_rows: win_size.1,
            row_offset: 0,
            column_offset: 0,
            render_x: 0,
        }
    }

    fn get_render_x(&self, row: &Row) -> usize {
        row.row_content[..self.cursor_x]
            .chars()
            .fold(0, |render_x, c| {
                if c == '\t' {
                    render_x + (TAB_STOP - 1) - (render_x % TAB_STOP) + 1
                } else {
                    render_x + 1
                }
            })
    }

    fn move_cursor(&mut self, direction: KeyCode, editor_rows: &EditorRows) {
        let number_of_rows = editor_rows.number_of_rows();

        match direction {
            KeyCode::Up => {
                self.cursor_y = self.cursor_y.saturating_sub(1);
            }
            KeyCode::Left => {
                if self.cursor_x != 0 {
                    self.cursor_x -= 1;
                } else if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                    self.cursor_x = editor_rows.get_row(self.cursor_y).len();
                }
            }
            KeyCode::Down => {
                if self.cursor_y < number_of_rows {
                    self.cursor_y += 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_y < number_of_rows {
                    if self.cursor_x < editor_rows.get_row(self.cursor_y).len() {
                        self.cursor_x += 1;
                    } else {
                        self.cursor_x = 0;
                        self.cursor_y += 1;
                    }
                }
            }
            KeyCode::Home => self.cursor_x = 0,
            KeyCode::End => {
                if self.cursor_y < number_of_rows {
                    self.cursor_x = editor_rows.get_row(self.cursor_y).len();
                }
            }
            _ => unimplemented!(),
        }

        let row_len = if self.cursor_y < number_of_rows {
            editor_rows.get_row(self.cursor_y).len()
        } else {
            0
        };

        self.cursor_x = cmp::min(self.cursor_x, row_len);
    }

    fn scroll(&mut self, editor_rows: &EditorRows) {
        self.render_x = 0;
        if self.cursor_y < editor_rows.number_of_rows() {
            self.render_x = self.get_render_x(editor_rows.get_editor_row(self.cursor_y))
        }

        // vertical scroll
        self.row_offset = cmp::min(self.row_offset, self.cursor_y);
        if self.cursor_y >= self.row_offset + self.screen_rows {
            self.row_offset = self.cursor_y - self.screen_rows + 1;
        }

        // horizontal scroll
        self.column_offset = cmp::min(self.column_offset, self.render_x);
        if self.render_x >= self.column_offset + self.screen_columns {
            self.column_offset = self.render_x - self.screen_columns + 1;
        }
    }
}

struct StatusMessage {
    message: Option<String>,
    set_time: Option<Instant>,
}

impl StatusMessage {
    fn new(initial_message: String) -> Self {
        Self {
            message: Some(initial_message),
            set_time: Some(Instant::now()),
        }
    }

    fn set_message(&mut self, message: String) {
        self.message = Some(message);
        self.set_time = Some(Instant::now())
    }

    fn message(&mut self) -> Option<&String> {
        self.set_time.and_then(|time| {
            if time.elapsed() > Duration::from_secs(5) {
                self.message = None;
                self.set_time = None;
                None
            } else {
                Some(self.message.as_ref().unwrap())
            }
        })
    }
}

// Output struct is used to handle the output to the
// terminal screen. This includes the ~ at the start of
// each line like Vim and also used to ensure that
// instead of multiple small writes happening each time,
// one big write happens (efficient)
struct Output {
    win_size: (usize, usize),
    editor_contents: EditorContents,
    cursor_controller: CursorController,
    editor_rows: EditorRows,
    status_message: StatusMessage,
    dirty: u64,
}

impl Output {
    fn new() -> Self {
        // Get window size of the current terminal screen
        // and instantiate the output with the window size
        // and the contents that will go there
        let win_size = terminal::size()
            .map(|(x, y)| (x as usize, y as usize - 2))
            .unwrap();
        Self {
            win_size,
            editor_contents: EditorContents::new(),
            cursor_controller: CursorController::new(win_size),
            editor_rows: EditorRows::new(),
            status_message: StatusMessage::new("Help: CTRL + s to Save | CTRL + q to Quit.".into()),
            dirty: 0,
        }
    }

    fn clear_screen() -> crossterm::Result<()> {
        // Associate function that will be called whenever
        // there is a need to clear screen and relocate the cursor
        // to the top left
        execute!(stdout(), terminal::Clear(ClearType::All))?;
        execute!(stdout(), cursor::MoveTo(0, 0))
    }

    fn insert_char(&mut self, ch: char) {
        if self.cursor_controller.cursor_y == self.editor_rows.number_of_rows() {
            self.editor_rows.insert_row()
        }

        self.editor_rows
            .get_editor_row_mut(self.cursor_controller.cursor_y)
            .insert_char(self.cursor_controller.cursor_x, ch);

        self.cursor_controller.cursor_x += 1;

        // tracks that file has been modified
        // counts the amount of changes
        self.dirty += 1;
    }

    fn draw_rows(&mut self) {
        // Draws each row in the terminal window based on the size
        // saved when initialized. Includes drawing the ~ at the start
        // of each row and also a welcome message at the horizontal center
        // of the screen, a third of the way down vertically.
        let screen_rows = self.win_size.1;
        let screen_columns = self.win_size.0;

        for i in 0..screen_rows {
            let file_row = i + self.cursor_controller.row_offset;

            if file_row >= self.editor_rows.number_of_rows() {
                if self.editor_rows.number_of_rows() == 0 && i == screen_rows / 3 {
                    let mut welcome = format!("Pound editor --- Version {}", VERSION);

                    if welcome.len() > screen_columns {
                        welcome.truncate(screen_columns)
                    }

                    let mut padding = (screen_columns - welcome.len()) / 2;
                    if padding != 0 {
                        self.editor_contents.push('~');
                        padding -= 1;
                    }
                    (0..padding).for_each(|_| self.editor_contents.push(' '));

                    self.editor_contents.push_str(&welcome);
                } else {
                    self.editor_contents.push('~');
                }
            } else {
                let row = self.editor_rows.get_render(file_row);
                let column_offset = self.cursor_controller.column_offset;

                let len = if row.len() < column_offset {
                    0
                } else {
                    let len = row.len() - column_offset;
                    if len > screen_columns {
                        screen_columns
                    } else {
                        len
                    }
                };

                let start = if len == 0 { 0 } else { column_offset };

                self.editor_contents.push_str(&row[start..start + len]);
            }
            queue!(
                self.editor_contents,
                terminal::Clear(ClearType::UntilNewLine)
            )
            .unwrap();

            self.editor_contents.push_str("\r\n");
        }
    }

    fn draw_status_bar(&mut self) {
        self.editor_contents
            .push_str(&style::Attribute::Reverse.to_string());

        let info = format!(
            "{} {} -- {} lines",
            self.editor_rows
                .filename
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("[No Name]"),
            if self.dirty > 0 { "(modified)" } else { "" },
            self.editor_rows.number_of_rows()
        );

        let info_len = cmp::min(info.len(), self.win_size.0);

        let line_info = format!(
            "{}/{}",
            self.cursor_controller.cursor_y + 1,
            self.editor_rows.number_of_rows()
        );

        self.editor_contents.push_str(&info[..info_len]);
        for i in info_len..self.win_size.0 {
            if self.win_size.0 - i == line_info.len() {
                self.editor_contents.push_str(&line_info);
                break;
            } else {
                self.editor_contents.push(' ')
            }
        }

        self.editor_contents.push_str("\r\n");
        self.editor_contents
            .push_str(&style::Attribute::Reset.to_string());
    }

    fn draw_message_bar(&mut self) {
        // Draws out any message passed in at the very bottom
        // of the screen
        queue!(
            self.editor_contents,
            terminal::Clear(ClearType::UntilNewLine)
        )
        .unwrap();

        if let Some(msg) = self.status_message.message() {
            self.editor_contents
                .push_str(&msg[..cmp::min(self.win_size.0, msg.len())]);
        }
    }

    fn refresh_screen(&mut self) -> crossterm::Result<()> {
        // 'queue' will queue commands to be run in the terminal
        // (provided by crossterm)
        // Hide the cursor before updates and relocate it to the top left
        // Show it back when update finishes
        // Also calls scroll
        self.cursor_controller.scroll(&self.editor_rows);
        queue!(self.editor_contents, cursor::Hide, cursor::MoveTo(0, 0))?;

        self.draw_rows();
        self.draw_status_bar();
        self.draw_message_bar();

        // Move the cursor to particular location based on
        // the cursor controller class
        let cursor_x = self.cursor_controller.render_x - self.cursor_controller.column_offset;
        let cursor_y = self.cursor_controller.cursor_y - self.cursor_controller.row_offset;

        queue!(
            self.editor_contents,
            cursor::MoveTo(cursor_x as u16, cursor_y as u16),
            cursor::Show,
        )?;
        self.editor_contents.flush()
    }

    fn move_cursor(&mut self, direction: KeyCode) {
        self.cursor_controller
            .move_cursor(direction, &self.editor_rows);
    }
}

// Reader struct is used to read keypresses by the user
struct Reader;

impl Reader {
    // Read the key pressed by the user and check every
    // 5 seconds for input
    fn read_key(&self) -> crossterm::Result<KeyEvent> {
        loop {
            if event::poll(Duration::from_millis(5000))? {
                if let Event::Key(event) = event::read()? {
                    return Ok(event);
                }
            }
        }
    }
}

// Used to store row content and row render
// content
#[derive(Default)]
struct Row {
    row_content: String,
    render: String,
}

impl Row {
    fn new(row_content: String, render: String) -> Self {
        Self {
            row_content,
            render,
        }
    }

    fn insert_char(&mut self, at: usize, ch: char) {
        self.row_content.insert(at, ch);
        EditorRows::render_row(self);
    }
}

const TAB_STOP: usize = 8;
// Used to store contents of rows in the
struct EditorRows {
    row_contents: Vec<Row>,
    filename: Option<PathBuf>,
}

impl EditorRows {
    fn new() -> Self {
        let mut arg = env::args();

        match arg.nth(1) {
            None => Self {
                row_contents: Vec::new(),
                filename: None,
            },
            Some(file) => Self::from_file(file.into()),
        }
    }

    fn render_row(row: &mut Row) {
        let mut index = 0;

        let capacity = row
            .row_content
            .chars()
            .fold(0, |acc, next| acc + if next == '\t' { TAB_STOP } else { 1 });

        row.render = String::with_capacity(capacity);
        row.row_content.chars().for_each(|c| {
            index += 1;
            if c == '\t' {
                row.render.push(' ');
                while index % TAB_STOP != 0 {
                    row.render.push(' ');
                    index += 1;
                }
            } else {
                row.render.push(c);
            }
        });
    }

    fn insert_row(&mut self) {
        self.row_contents.push(Row::default());
    }

    fn get_editor_row_mut(&mut self, at: usize) -> &mut Row {
        &mut self.row_contents[at]
    }

    fn from_file(file: PathBuf) -> Self {
        let file_contents = fs::read_to_string(&file).expect("Unable to read file");

        Self {
            filename: Some(file),
            row_contents: file_contents
                .lines()
                .map(|it| {
                    let mut row = Row::new(it.into(), String::new());
                    Self::render_row(&mut row);
                    row
                })
                .collect(),
        }
    }

    fn get_render(&self, at: usize) -> &String {
        &self.row_contents[at].render
    }

    fn get_editor_row(&self, at: usize) -> &Row {
        &self.row_contents[at]
    }

    fn number_of_rows(&self) -> usize {
        self.row_contents.len()
    }

    fn get_row(&self, at: usize) -> &str {
        &self.row_contents[at].row_content
    }

    fn save(&self) -> io::Result<usize> {
        match &self.filename {
            None => Err(io::Error::new(
                io::ErrorKind::Other,
                "no file name specified",
            )),
            Some(name) => {
                let mut file = fs::OpenOptions::new().write(true).create(true).open(name)?;
                let contents: String = self
                    .row_contents
                    .iter()
                    .map(|it| it.row_content.as_str())
                    .collect::<Vec<&str>>()
                    .join("\n");
                file.set_len(contents.len() as u64)?;
                file.write_all(contents.as_bytes())?;
                Ok(contents.as_bytes().len())
            }
        }
    }
}

// The actual text editor struct, includes the key
// press reader and also the output that will be displayed
// in this text editor.
struct Editor {
    reader: Reader,
    output: Output,
    quit_times: u8,
}

const QUIT_TIMES: u8 = 2;

impl Editor {
    fn new() -> Self {
        Self {
            reader: Reader,
            output: Output::new(),
            quit_times: QUIT_TIMES,
        }
    }

    fn process_keypress(&mut self) -> crossterm::Result<bool> {
        // Check what key is pressed by the user
        // quit editor if Ctrl+q is pressed
        // Ctrl, Shift etc are called Key Modifiers
        match self.reader.read_key()? {
            KeyEvent {
                code: KeyCode::Char('q'),
                modifiers: event::KeyModifiers::CONTROL,
            } => {
                if self.output.dirty > 0 && self.quit_times > 0 {
                    self.output.status_message.set_message(format!(
                        "WARNING! File has unsaved changes. Press Ctrl+q {} more times to quit.",
                        self.quit_times
                    ));
                    // decrement quit times each time Ctrl+q is pressed
                    self.quit_times -= 1;
                    return Ok(true);
                }

                return Ok(false);
            }
            KeyEvent {
                code:
                    direction @ (KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End),
                modifiers: event::KeyModifiers::NONE,
            } => self.output.move_cursor(direction),
            KeyEvent {
                // Used to move to top and bottom of page instantly
                code: val @ (KeyCode::PageUp | KeyCode::PageDown),
                modifiers: event::KeyModifiers::NONE,
            } => {
                if matches!(val, KeyCode::PageUp) {
                    self.output.cursor_controller.cursor_y =
                        self.output.cursor_controller.row_offset
                } else {
                    self.output.cursor_controller.cursor_y = cmp::min(
                        self.output.win_size.1 + self.output.cursor_controller.row_offset - 1,
                        self.output.editor_rows.number_of_rows(),
                    );
                }
            }
            KeyEvent {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::CONTROL,
            } => self.output.editor_rows.save().map(|len| {
                self.output
                    .status_message
                    .set_message(format!("{} bytes written to disk", len));
                self.output.dirty = 0;
            })?,
            KeyEvent {
                // Used to handle a user input to the text 'editor'
                // Handles any other key pressed by the user
                // That isn't already mapped above.
                // Also prevents modifiers like Ctrl to be used to enter
                // characters (Ex: Ctrl + X shouldn't insert X).
                code: code @ (KeyCode::Char(..) | KeyCode::Tab),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            } => self.output.insert_char(match code {
                KeyCode::Tab => '\t',
                KeyCode::Char(ch) => ch,
                _ => unreachable!(),
            }),
            _ => {}
        }

        Ok(true)
    }

    fn run(&mut self) -> crossterm::Result<bool> {
        // When editor is run, refresh the screen first
        // then start processing key presses by user
        self.output.refresh_screen()?;
        self.process_keypress()
    }
}

// Used to store contents of editor for one big write
// instead of many smaller writes
struct EditorContents {
    content: String,
}

impl EditorContents {
    fn new() -> Self {
        Self {
            content: String::new(),
        }
    }

    fn push(&mut self, ch: char) {
        self.content.push(ch)
    }

    fn push_str(&mut self, string: &str) {
        self.content.push_str(string)
    }
}

impl io::Write for EditorContents {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match std::str::from_utf8(buf) {
            Ok(s) => {
                self.content.push_str(s);
                Ok(s.len())
            }
            Err(_) => Err(io::ErrorKind::WriteZero.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let out = write!(stdout(), "{}", self.content);
        stdout().flush()?;
        self.content.clear();
        out
    }
}

fn main() -> crossterm::Result<()> {
    let _clean_up = Cleanup;
    terminal::enable_raw_mode()?;

    let mut editor = Editor::new();
    while editor.run()? {}
    Ok(())
}
