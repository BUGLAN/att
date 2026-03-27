# Design System Document

## 1. Overview & Creative North Star: "The Digital Obsidian"

This design system is engineered for precision and focus. Moving away from the cluttered utility of standard clipboard managers, we embrace **"The Digital Obsidian"**—a creative North Star that treats the UI as a high-end, monolithic piece of hardware. It is inspired by professional audio engineering racks and technical transcription suites.

The experience is defined by deep, void-like blacks contrasted with a singular, luminescent amber light source. By utilizing **tonal layering** and **intentional asymmetry**, we break the "template" feel. Content isn't just placed on a grid; it is carved out of the darkness. The primary language, Simplified Chinese, is treated with editorial gravitas, ensuring that dense logograms remain legible through high-contrast typography and generous spatial breathing room.

---

## 2. Colors

The color palette is a study in restrained luxury. We use a "Super-Black" foundation to minimize eye strain and maximize the "glow" of the muted gold accents.

### Color Tokens
*   **Background / Surface:** `#131313` (The void foundation)
*   **Primary (Gold):** `#ffc557` (The active state, used for progress and key focus)
*   **Primary Container:** `#e8a70f` (Used for subtle tonal highlights in active elements)
*   **Surface Container (High):** `#2a2a2a` (Card backgrounds)
*   **On-Surface (Text):** `#e5e2e1` (High-contrast readability)
*   **On-Surface Variant:** `#d0c5af` (Secondary metadata and labels)

### The "No-Line" Rule
Standard 1px solid borders are prohibited for sectioning. Structural boundaries must be defined solely by background shifts. For example, a sidebar using `surface-container-low` should sit directly against the `surface` background without a stroke. Separation is a result of light (tonal value), not ink.

### The "Glass & Gradient" Rule
To add soul to the technical aesthetic, use semi-transparent surfaces (`surface-container-high` at 80% opacity) with a `backdrop-blur` of 20px for floating modals or context menus. Main CTAs should utilize a subtle linear gradient from `primary` to `primary-container` to mimic a physical illuminated button.

---

## 3. Typography

The system utilizes a dual-font approach to balance technical precision with modern elegance.

*   **Display/Headline (Manrope):** Used for large numerical data (e.g., clip timestamps) and main headers. It provides a geometric, technical feel.
*   **Body/Title (Inter / System Sans-Serif):** Optimized for Simplified Chinese. We prioritize the "Inter" family for English characters and high-quality sans-serif (such as PingFang SC) for Chinese to ensure uniform stroke weights.

### Typographic Hierarchy
*   **Headline-SM (1.5rem):** Reserved for primary view titles (e.g., "历史记录").
*   **Title-MD (1.125rem):** For clipboard content headers.
*   **Label-MD (0.75rem):** For metadata like "0:03", "Yesterday", or status tags. These should use `on-surface-variant` to maintain a quiet hierarchy.

---

## 4. Elevation & Depth

In this system, depth is not achieved with shadows, but through **Tonal Layering**.

*   **The Layering Principle:** Stacking follows a "Lower to Higher" logic.
    *   **Base Layer:** `surface` (#131313)
    *   **Section Layer:** `surface-container-low` (#1c1b1b)
    *   **Interactive Cards:** `surface-container-high` (#2a2a2a)
*   **The Ghost Border:** If a boundary is required for accessibility (e.g., in focused input states), use the `outline-variant` token (#4d4635) at 20% opacity. This creates a "glint" on the edge rather than a hard line.
*   **Ambient Shadows:** For floating Electron windows or tooltips, use an ultra-diffused shadow: `box-shadow: 0 12px 40px rgba(0, 0, 0, 0.6);`.

---

## 5. Components

### Buttons
*   **Primary (Action):** Rounded `md` (0.375rem). Background is the Gold gradient. Text is `on-primary` (#422d00).
*   **Ghost (Secondary):** No background. `outline-variant` at 20% opacity for the border. Gold text only on hover.

### Clipboard Cards
*   **Structure:** No divider lines between items. Use `spacing-4` (0.9rem) vertical margin to separate entries.
*   **States:** Default state uses `surface-container-low`. The "Active/Copied" state utilizes a `primary` (Gold) left-accent bar (2px wide) and a subtle shift to `surface-container-high`.

### Input Fields
*   **Visuals:** Inset appearance using `surface-container-lowest`. 
*   **Focus:** The border transitions from 0% to 40% opacity of the `primary` Gold token. No "glow" effect—keep it sharp and technical.

### Chips (Tags)
*   Used for "已优化" (Optimized) or "文本" (Text) indicators.
*   Small, `sm` rounded corners. Background: `primary_container` at 15% opacity. Text: `primary`.

---

## 6. Do's and Don'ts

### Do
*   **Do** use asymmetrical layouts. Let the transcriptions have breathing room on the right while technical meta-data stays pinned to the left.
*   **Do** use the Spacing Scale strictly. All margins and paddings must be multiples of the `0.2rem` (Token 1) base.
*   **Do** favor `on-surface-variant` for non-essential information to keep the interface feeling calm and professional.

### Don't
*   **Don't** use pure white (#FFFFFF). It will "bloom" painfully on the black background. Use `on-surface` (#e5e2e1) instead.
*   **Don't** use standard "Material Design" ripples. Use a simple, subtle background-color opacity shift for hover/active states.
*   **Don't** use vertical dividers. Use the `surface-container` shifts to define the sidebar vs. the main content area.