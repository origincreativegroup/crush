# Regenerates fixtures/golden/. Run inside reference/.venv on the Mac.
CLIPS := $(wildcard ../fixtures/clips/*.mp4 ../fixtures/clips/*.mov)
QUERIES := "a person talking to camera" "wide shot of a building" "close up of hands" "car driving on a road" "text on screen"

golden:
	mkdir -p ../fixtures/golden
	@for c in $(CLIPS); do n=$$(basename $$c | cut -d. -f1); \
	  python reference_scenes.py $$c > ../fixtures/golden/$$n.scenes.json; \
	  python reference_transcribe.py $$c > ../fixtures/golden/$$n.transcript.json; \
	  ffmpeg -y -loglevel error -ss 1.0 -i $$c -frames:v 1 ../fixtures/golden/$$n.frame.png; \
	  python reference_embed.py image ../fixtures/golden/$$n.frame.png --dump-tensor > ../fixtures/golden/$$n.image.json; done
	@i=0; for q in $(QUERIES); do i=$$((i+1)); python reference_embed.py text "$$q" > ../fixtures/golden/text$$i.json; done
	@echo "golden regenerated — commit with a message saying WHY"
