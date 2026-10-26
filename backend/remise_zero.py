import os
import subprocess
import sys

def main():
    db_file = os.path.join(os.path.dirname(__file__), 'db.json')

    print("Réinitialisation du logiciel...")

    # Supprimer le fichier de base de données JSON s'il existe
    if os.path.exists(db_file):
        try:
            os.remove(db_file)
            print(f"Fichier {db_file} supprimé.")
        except Exception as e:
            print(f"Erreur lors de la suppression de {db_file}: {e}")
            sys.exit(1)
    else:
        print("Aucun fichier de base de données trouvé. Création d'une nouvelle base.")

    # Exécuter init-db.js pour recréer la structure et l'admin
    try:
        init_script = os.path.join(os.path.dirname(__file__), 'init-db.js')
        result = subprocess.run(['node', init_script], capture_output=True, text=True, check=True)
        print(result.stdout)
        print("Réinitialisation terminée avec succès.")
    except subprocess.CalledProcessError as e:
        print(f"Erreur lors de l'initialisation de la base de données: {e.stderr}")
        sys.exit(1)

if __name__ == "__main__":
    main()
