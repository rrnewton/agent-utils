// The page's one way to turn a server-rendered string into nodes: a `<template>`'s
// parse, its child nodes appended. Only what the server rendered (a message's
// `content_html`, a Discuss answer's `text_html`) ever reaches it — never raw text.

/** Insert `contentHtml`, already rendered and sanitized by the server, into `parent`. */
export function insertSanitizedHtml(parent: HTMLElement, contentHtml: string): void {
  const template = document.createElement("template");
  template.innerHTML = contentHtml;
  parent.append(...template.content.childNodes);
}
