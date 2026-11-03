//! Un'area che segnala il tasto destro **con il punto del clic**.
//!
//! In iced 0.14 l'evento di pressione del tasto destro non porta la
//! posizione: solo i widget personalizzati la vedono, nel `Cursor` che
//! ricevono. Il menù contestuale si apre **dove si è cliccato**, quindi
//! questa area pubblica il punto insieme al messaggio, e lascia passare
//! tutto il resto al contenuto.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::mouse::{self, Button, Cursor};
use iced::{Element, Event, Length, Point, Rectangle, Size};

/// Area che pubblica un messaggio al tasto destro, col punto del clic.
///
/// Il soggetto (`S`) è ciò su cui si è cliccato — la scheda, il nodo
/// dell'albero — e resta **fuori** dal widget: il widget non sa che cosa
/// sia, sa solo restituirlo al costruttore del messaggio.
pub struct RightClickArea<'a, M, S> {
    content: Element<'a, M>,
    subject: Option<S>,
    on_right_press: fn(Point, &S) -> M,
}

impl<'a, M, S> RightClickArea<'a, M, S> {
    pub fn new(content: Element<'a, M>, subject: S, on_right_press: fn(Point, &S) -> M) -> Self {
        Self {
            content,
            subject: Some(subject),
            on_right_press,
        }
    }
}

impl<'a, M: 'a, S: Clone + 'a> From<RightClickArea<'a, M, S>> for Element<'a, M> {
    fn from(area: RightClickArea<'a, M, S>) -> Self {
        Element::new(area)
    }
}

impl<'a, M, S: Clone> Widget<M, iced::Theme, iced::Renderer> for RightClickArea<'a, M, S> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        // Il figlio è sempre uno (`children` lo dichiara): se mancasse,
        // l'area non ha nulla da disporre e resta vuota.
        match tree.children.first_mut() {
            Some(figlio) => self
                .content
                .as_widget_mut()
                .layout(figlio, renderer, limits),
            None => layout::Node::new(limits.max()),
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        // Il tasto destro è dell'area, non del contenuto: si intercetta
        // **prima** di delegare, e con il punto esatto del clic.
        if let Event::Mouse(mouse::Event::ButtonPressed(Button::Right)) = event
            && let Some(punto) = cursor.position_over(layout.bounds())
            && let Some(soggetto) = self.subject.clone()
        {
            shell.publish((self.on_right_press)(punto, &soggetto));
            shell.capture_event();
            return;
        }
        let Some(figlio) = tree.children.first_mut() else {
            return;
        };
        self.content.as_widget_mut().update(
            figlio, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        match tree.children.first() {
            Some(figlio) => self
                .content
                .as_widget()
                .mouse_interaction(figlio, layout, cursor, viewport, renderer),
            None => mouse::Interaction::None,
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        renderer_style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
    ) {
        let Some(figlio) = tree.children.first() else {
            return;
        };
        self.content.as_widget().draw(
            figlio,
            renderer,
            theme,
            renderer_style,
            layout,
            cursor,
            viewport,
        );
    }
}
