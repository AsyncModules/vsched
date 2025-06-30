OUTPUT_ARCH(%ARCH%)

SECTIONS
{
	. = SIZEOF_HEADERS;

	.hash		: { *(.hash) }			:text
	.gnu.hash	: { *(.gnu.hash) }
	.dynsym		: { *(.dynsym) }
	.dynstr		: { *(.dynstr) }
	.gnu.version	: { *(.gnu.version) }
	.gnu.version_d	: { *(.gnu.version_d) }
	.gnu.version_r	: { *(.gnu.version_r) }

	.dynamic	: { *(.dynamic) }		:text	:dynamic

	.rodata		: {
		*(.rodata .rodata.* .gnu.linkonce.r.*)
		*(.got.plt) *(.got)
		*(.data .data.* .gnu.linkonce.d.*)
		*(.dynbss)
		*(.bss .bss.* .gnu.linkonce.b.*)
	}

	.note		: { *(.note.*) }		:text	:note

	.eh_frame_hdr	: { *(.eh_frame_hdr) }		:text	:eh_frame_hdr
	.eh_frame	: { KEEP (*(.eh_frame)) }	:text

	/*
	 * Text is well-separated from actual data: there's plenty of
	 * stuff that isn't used at runtime in between.
	 */
	. = ALIGN(16);
	.text		: { *(.text .text.*) }		:text

	. = ALIGN(4);
	.alternative : {
		*(.alternative)
	}
}

/*
 * We must supply the ELF program headers explicitly to get just one
 * PT_LOAD segment, and set the flags explicitly to make segments read-only.
 */
PHDRS
{
	text		PT_LOAD		FLAGS(5) FILEHDR PHDRS; /* PF_R|PF_X */
	dynamic		PT_DYNAMIC	FLAGS(4);		/* PF_R */
	note		PT_NOTE		FLAGS(4);		/* PF_R */
	eh_frame_hdr	PT_GNU_EH_FRAME;
}