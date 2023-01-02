use ::crossterm::event::*;
use ::crossterm::terminal::ClearType;
use ::crossterm::{cursor, event, execute, queue, terminal};
use std::cmp;
use std::io::{self, stdout, Write};
use std::time::Duration;

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
}

impl CursorController {
    fn new(win_size: (usize, usize)) -> CursorController {
        Self {
            cursor_x: 0,
            cursor_y: 0,
            screen_columns: win_size.0,
            screen_rows: win_size.1,
        }
    }

    fn move_cursor(&mut self, direction: KeyCode) {
        match direction {
            KeyCode::Up => {
                self.cursor_y = self.cursor_y.saturating_sub(1);
            }
            KeyCode::Left => {
                self.cursor_x = self.cursor_x.saturating_sub(1);
            }
            KeyCode::Down => {
                if self.cursor_y != self.screen_rows - 1 {
                    self.cursor_y += 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_x != self.screen_columns - 1 {
                    self.cursor_x += 1;
                }
            }
            KeyCode::Home => self.cursor_x = 0,
            KeyCode::End => self.cursor_x = self.screen_columns - 1,
            _ => unimplemented!(),
        }
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
}

impl Output {
    fn new() -> Self {
        // Get window size of the current terminal screen
        // and instantiate the output with the window size
        // and the contents that will go there
        let win_size = terminal::size()
            .map(|(x, y)| (x as usize, y as usize))
            .unwrap();
        Self {
            win_size,
            editor_contents: EditorContents::new(),
            cursor_controller: CursorController::new(win_size),
            editor_rows: EditorRows::new(),
        }
    }

    fn clear_screen() -> crossterm::Result<()> {
        // Associate function that will be called whenever
        // there is a need to clear screen and relocate the cursor
        // to the top left
        execute!(stdout(), terminal::Clear(ClearType::All))?;
        execute!(stdout(), cursor::MoveTo(0, 0))
    }

    fn draw_rows(&mut self) {
        // Draws each row in the terminal window based on the size
        // saved when initialized. Includes drawing the ~ at the start
        // of each row and also a welcome message at the horizontal center
        // of the screen, a third of the way down vertically.
        let screen_rows = self.win_size.1;
        let screen_columns = self.win_size.0;

        for i in 0..screen_rows {
            if i >= self.editor_rows.number_of_rows() {
                if i == screen_rows / 3 {
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
                let len = cmp::min(self.editor_rows.get_row().len(), screen_columns);

                self.editor_contents
                    .push_str(&self.editor_rows.get_row()[..len])
            }
            queue!(
                self.editor_contents,
                terminal::Clear(ClearType::UntilNewLine)
            )
            .unwrap();

            if i < screen_rows - 1 {
                self.editor_contents.push_str("\r\n");
            }
        }
    }

    fn refresh_screen(&mut self) -> crossterm::Result<()> {
        // 'queue' will queue commands to be run in the terminal
        // (provided by crossterm)
        // Hide the cursor before updates and relocate it to the top left
        // Show it back when update finishes
        queue!(self.editor_contents, cursor::Hide, cursor::MoveTo(0, 0))?;

        self.draw_rows();

        // Move the cursor to particular location based on
        // the cursor controller class
        let cursor_x = self.cursor_controller.cursor_x;
        let cursor_y = self.cursor_controller.cursor_y;

        queue!(
            self.editor_contents,
            cursor::MoveTo(cursor_x as u16, cursor_y as u16),
            cursor::Show,
        )?;
        self.editor_contents.flush()
    }

    fn move_cursor(&mut self, direction: KeyCode) {
        self.cursor_controller.move_cursor(direction);
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

// Used to store contents of rows in the
struct EditorRows {
    row_contents: Vec<Box<str>>,
}

impl EditorRows {
    fn new() -> Self {
        Self {
            row_contents: vec!["Hello World".into()],
        }
    }

    fn number_of_rows(&self) -> usize {
        1
    }

    fn get_row(&self) -> &str {
        &self.row_contents[0]
    }
}

// The actual text editor struct, includes the key
// press reader and also the output that will be displayed
// in this text editor.
struct Editor {
    reader: Reader,
    output: Output,
}

impl Editor {
    fn new() -> Self {
        Self {
            reader: Reader,
            output: Output::new(),
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
            } => return Ok(false),
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
            } => (0..self.output.win_size.1).for_each(|_| {
                self.output.move_cursor(if matches!(val, KeyCode::PageUp) {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                });
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
