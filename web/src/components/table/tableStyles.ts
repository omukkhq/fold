/** Tables separate rows by a light zebra fill and space, never by row borders. */
export const TABLE_ROW = "even:bg-sunken/45";

/** A row that opens something, such as a proposal, also tints under the pointer. */
export const TABLE_LINK_ROW = `${TABLE_ROW} hover:bg-sunken`;

/** A column head: 13px semibold sentence case in the secondary tone, never uppercase. */
export const TABLE_HEAD =
  "px-12 py-8 text-left font-title text-13 leading-tight font-semibold text-text-2";

export const TABLE_CELL = "px-12 py-10 align-top text-14";
